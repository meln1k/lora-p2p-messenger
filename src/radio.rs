use defmt::{debug, info, warn};
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDevice;
use embassy_futures::select::{Either, select};
use lora_p2p_messenger::{
    commands::Command,
    constants::*,
    events::Event,
    messages::{InboundRadioMessage, MAX_MESSAGE_LEN},
    runtime::OUTBOUND_MESSAGES,
};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Receiver, Sender},
    pubsub::DynImmediatePublisher,
};
use embassy_time::{Delay, Duration, Ticker, Timer};
use esp_hal::{
    Async,
    gpio::{Input, Output},
    spi::master::Spi,
};
use lora_phy::{
    LoRa,
    RxMode,
    mod_params::{Bandwidth, CodingRate, SpreadingFactor},
    sx127x,
    sx127x::{Sx127x, Sx1276},
};

use crate::lora_interface::TBeamSx127xInterfaceVariant;

const OUTPUT_POWER: i32 = 10;
const ULTRA_RX_LEN: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Messaging,
    UltraRx,
    UltraTx,
}

#[embassy_executor::task]
pub async fn task(
    lora_rst: Output<'static>,
    mut ldo_en: Output<'static>,
    ctl_lna: Output<'static>,
    lora_dio0: Input<'static>,
    lora_dio1: Input<'static>,
    spi_device: SpiDevice<'static, CriticalSectionRawMutex, Spi<'static, Async>, Output<'static>>,
    radio_commands: Receiver<'static, CriticalSectionRawMutex, Command, 8>,
    inbound_radio_messages: DynImmediatePublisher<'static, InboundRadioMessage>,
    app_events: Sender<'static, CriticalSectionRawMutex, Event, 8>,
) {
    info!("radio: task started");
    let outbound_messages = OUTBOUND_MESSAGES.receiver();

    ldo_en.set_high();
    Timer::after_millis(50).await;

    let sx127x_config = sx127x::Config {
        chip: Sx1276,
        tcxo_used: true,
        tx_boost: true,
        rx_boost: false,
    };

    let iv = TBeamSx127xInterfaceVariant::new(lora_rst, lora_dio0, lora_dio1, Some(ctl_lna), None)
        .unwrap();

    let mut lora = LoRa::with_syncword(
        Sx127x::new(spi_device, iv, sx127x_config),
        LORA_SYNCWORD,
        Delay,
    )
    .await
    .unwrap();
    info!("radio: LoRa init complete (syncword={})", LORA_SYNCWORD);

    // Profile A: ultra long range
    // SF12 / BW 7.8kHz equivalent (Bandwidth::_7KHz enum, 7810 Hz), implicit header, no CRC.
    let ultra_mod = lora
        .create_modulation_params(
            SpreadingFactor::_12,
            Bandwidth::_7KHz,
            CodingRate::_4_8,
            LORA_FREQUENCY_HZ,
        )
        .unwrap();
    let mut ultra_tx_params = lora
        .create_tx_packet_params(
            LORA_PREAMBLE_LEN,
            true,  // implicit header
            false, // crc
            false,
            &ultra_mod,
        )
        .unwrap();
    let ultra_rx_params = lora
        .create_rx_packet_params(
            LORA_PREAMBLE_LEN,
            true, // implicit header
            ULTRA_RX_LEN,
            false, // crc
            false,
            &ultra_mod,
        )
        .unwrap();

    // Profile B: messaging
    // SF10 / BW 20.8kHz (Bandwidth::_20KHz enum), explicit header, CRC enabled.
    let msg_mod = lora
        .create_modulation_params(
            SpreadingFactor::_10,
            Bandwidth::_20KHz,
            CodingRate::_4_5,
            LORA_FREQUENCY_HZ,
        )
        .unwrap();
    let mut msg_tx_params = lora
        .create_tx_packet_params(
            LORA_MSG_PREAMBLE_LEN,
            false, // explicit header
            true,  // crc
            false,
            &msg_mod,
        )
        .unwrap();
    let msg_rx_params = lora
        .create_rx_packet_params(
            LORA_MSG_PREAMBLE_LEN,
            false, // explicit header
            MAX_MESSAGE_LEN as u8,
            true, // crc
            false,
            &msg_mod,
        )
        .unwrap();

    let mut receiving_buffer = [0u8; MAX_MESSAGE_LEN];

    let mut enabled = true;
    let mut mode = Mode::Messaging;
    let mut ultra_counter: u16 = 0;
    let mut ultra_ticker: Option<Ticker> = None;

    app_events.send(Event::RadioEnabled).await;
    info!("radio: enabled, initial mode=Messaging");

    loop {
        while let Ok(cmd) = radio_commands.try_receive() {
            match cmd {
                Command::DisableRadio if enabled => {
                    info!("radio: cmd DisableRadio");
                    lora.sleep(false).await.ok();
                    ldo_en.set_low();
                    enabled = false;
                    app_events.send(Event::RadioDisabled).await;
                    info!("radio: disabled");
                }
                Command::EnableRadio if !enabled => {
                    info!("radio: cmd EnableRadio");
                    ldo_en.set_high();
                    Timer::after_millis(50).await;
                    if lora.init().await.is_ok() {
                        enabled = true;
                        app_events.send(Event::RadioEnabled).await;
                        info!("radio: re-enabled");
                    } else {
                        warn!("radio: re-init failed");
                    }
                }
                Command::StartRxMode if enabled => {
                    info!("radio: cmd StartRxMode -> UltraRx");
                    mode = Mode::UltraRx;
                    app_events.send(Event::RxModeOn).await;
                }
                Command::StopRxMode if enabled && matches!(mode, Mode::UltraRx) => {
                    info!("radio: cmd StopRxMode -> Messaging");
                    mode = Mode::Messaging;
                    app_events.send(Event::RadioStandby).await;
                }
                Command::StartTxMode if enabled => {
                    info!("radio: cmd StartTxMode -> UltraTx");
                    mode = Mode::UltraTx;
                    ultra_ticker = Some(Ticker::every(Duration::from_secs(5)));
                    app_events.send(Event::TxModeOn).await;
                }
                Command::StopTxMode if enabled && matches!(mode, Mode::UltraTx) => {
                    info!("radio: cmd StopTxMode -> Messaging");
                    mode = Mode::Messaging;
                    ultra_ticker = None;
                    app_events.send(Event::RadioStandby).await;
                }
                Command::StartMessagingMode if enabled => {
                    info!("radio: cmd StartMessagingMode -> Messaging");
                    mode = Mode::Messaging;
                    ultra_ticker = None;
                    app_events.send(Event::RadioStandby).await;
                }
                _ => {}
            }
        }

        if !enabled {
            debug!("radio: disabled loop idle");
            Timer::after(Duration::from_millis(100)).await;
            continue;
        }

        match mode {
            Mode::Messaging => {
                if let Ok(msg) = outbound_messages.try_receive() {
                    info!("radio: messaging TX len={}", msg.len());
                    if lora
                        .prepare_for_tx(&msg_mod, &mut msg_tx_params, OUTPUT_POWER, msg.as_slice())
                        .await
                        .is_ok()
                        && lora.tx().await.is_ok()
                    {
                        info!("radio: messaging TX done");
                        app_events.send(Event::TxDone { counter: 0 }).await;
                    } else {
                        warn!("radio: messaging TX failed");
                    }
                    continue;
                }

                if lora
                    .prepare_for_rx(RxMode::Single(6), &msg_mod, &msg_rx_params)
                    .await
                    .is_err()
                {
                    warn!("radio: messaging prepare_for_rx failed");
                    Timer::after(Duration::from_millis(30)).await;
                    continue;
                }

                if let Ok((received_len, packet_status)) =
                    lora.rx(&msg_rx_params, &mut receiving_buffer).await
                {
                    let len = received_len as usize;
                    if len > 0 {
                        info!(
                            "radio: messaging RX len={} rssi={} snr={}",
                            len, packet_status.rssi, packet_status.snr
                        );
                        let payload = &receiving_buffer[..len];
                        let mut cloned = heapless::Vec::<u8, MAX_MESSAGE_LEN>::new();
                        let _ = cloned.extend_from_slice(payload);
                        inbound_radio_messages.publish_immediate(InboundRadioMessage {
                            payload: cloned,
                            rssi: packet_status.rssi,
                            snr: packet_status.snr,
                        });
                        app_events
                            .send(Event::RxSuccess {
                                counter: 0,
                                rssi: packet_status.rssi,
                                snr: packet_status.snr,
                            })
                            .await;
                    }
                } else {
                    debug!("radio: messaging RX timeout/error");
                }
            }
            Mode::UltraRx => {
                if lora
                    .prepare_for_rx(
                        RxMode::Single(LORA_PREAMBLE_LEN * 2),
                        &ultra_mod,
                        &ultra_rx_params,
                    )
                    .await
                    .is_err()
                {
                    warn!("radio: ultra-rx prepare_for_rx failed");
                    Timer::after(Duration::from_millis(30)).await;
                    continue;
                }

                if let Ok((received_len, packet_status)) =
                    lora.rx(&ultra_rx_params, &mut receiving_buffer).await
                {
                    if received_len >= 2 {
                        let counter =
                            u16::from_le_bytes([receiving_buffer[0], receiving_buffer[1]]);
                        info!(
                            "radio: ultra-rx packet counter={} rssi={} snr={}",
                            counter, packet_status.rssi, packet_status.snr
                        );
                        app_events
                            .send(Event::RxSuccess {
                                counter,
                                rssi: packet_status.rssi,
                                snr: packet_status.snr,
                            })
                            .await;
                    }
                } else {
                    debug!("radio: ultra-rx timeout/error");
                }
            }
            Mode::UltraTx => {
                if let Some(ticker) = ultra_ticker.as_mut() {
                    match select(ticker.next(), Timer::after(Duration::from_millis(25))).await {
                        Either::First(_) => {
                            let payload = ultra_counter.to_le_bytes();
                            info!("radio: ultra-tx send counter={}", ultra_counter);
                            if lora
                                .prepare_for_tx(
                                    &ultra_mod,
                                    &mut ultra_tx_params,
                                    OUTPUT_POWER,
                                    &payload,
                                )
                                .await
                                .is_ok()
                                && lora.tx().await.is_ok()
                            {
                                info!("radio: ultra-tx done counter={}", ultra_counter);
                                app_events
                                    .send(Event::TxDone {
                                        counter: ultra_counter,
                                    })
                                    .await;
                                ultra_counter = ultra_counter.wrapping_add(1);
                            } else {
                                warn!("radio: ultra-tx failed counter={}", ultra_counter);
                            }
                        }
                        Either::Second(_) => {}
                    }
                } else {
                    debug!("radio: ultra-tx ticker missing, recreating");
                    ultra_ticker = Some(Ticker::every(Duration::from_secs(5)));
                }
            }
        }
    }
}

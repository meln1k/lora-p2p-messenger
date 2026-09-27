use alloc::format;

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use lora_p2p_messenger::{
    app::{AppState, Mode, On, RadioState, Screen},
    constants::{LORA_FREQUENCY_HZ, LORA_PREAMBLE_LEN, LORA_SYNCWORD},
    web,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use embedded_graphics::{
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use esp_hal::{Async, i2c::master::I2c};
use heapless::String;
use mousefood::prelude::*;
use oled_async::{Builder, displays::sh1106::Sh1106_128_64, prelude::*};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};
use ratatui::{
    Frame,
    Terminal,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Block, Paragraph, Widget},
};

use crate::alloc::{string::ToString, vec};

fn draw_wifi_overlay(
    display: &mut impl DrawTarget<Color = BinaryColor>,
    wifi_client_connected: bool,
) {
    let light = PrimitiveStyle::with_fill(BinaryColor::On);
    let dark = PrimitiveStyle::with_fill(BinaryColor::Off);

    let mut temp = [0u8; Version::MAX.buffer_len()];
    let mut out = [0u8; Version::MAX.buffer_len()];
    let mut qr_payload = String::<96>::new();
    if wifi_client_connected {
        let url = web::ap_url();
        let _ = qr_payload.push_str(url.as_str());
    } else {
        let creds = web::ap_wifi_qr_payload();
        let _ = qr_payload.push_str(creds.as_str());
    }

    if let Ok(qr) = QrCode::encode_text(
        qr_payload.as_str(),
        &mut temp,
        &mut out,
        QrCodeEcc::Low,
        Version::MIN,
        Version::MAX,
        None,
        true,
    ) {
        let size = qr.size();
        let module_px = 1i32;
        let qr_px = size * module_px;
        let quiet_zone = 2i32;
        let right_margin = quiet_zone;
        let bottom_margin = quiet_zone;
        let left = 128 - qr_px - right_margin;
        let top = 64 - qr_px - bottom_margin;

        let _ = Rectangle::new(
            Point::new(left - quiet_zone, top - quiet_zone),
            Size::new(
                (qr_px + quiet_zone * 2) as u32,
                (qr_px + quiet_zone * 2) as u32,
            ),
        )
        .into_styled(light)
        .draw(display);

        for y in 0..size {
            for x in 0..size {
                if qr.get_module(x, y) {
                    let _ = Rectangle::new(
                        Point::new(left + x * module_px, top + y * module_px),
                        Size::new(module_px as u32, module_px as u32),
                    )
                    .into_styled(dark)
                    .draw(display);
                }
            }
        }
    }
}

#[embassy_executor::task]
pub async fn display_task(
    i2c: I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>,
    ui_signal: &'static Signal<CriticalSectionRawMutex, AppState>,
) {
    // Create display interface
    let di = display_interface_i2c::I2CInterface::new(i2c, 0x3C, 0x40);

    // SH1106 128x64 display
    let raw_disp = Builder::new(Sh1106_128_64 {}).connect(di);

    let mut display: GraphicsMode<_, _, { 128 * 64 / 8 }> = raw_disp.into();

    display.init().await.expect("failed to initialize display");
    display.clear();
    display.flush().await.expect("failed to flush display");

    let mut terminal = {
        let backend = EmbeddedBackend::new(&mut display, EmbeddedBackendConfig::default());
        Terminal::new(backend).unwrap()
    };

    defmt::info!("terminal created");

    loop {
        let state = ui_signal.wait().await;
        terminal
            .draw(|f: &mut Frame| f.render_widget(AppView(&state), f.area()))
            .ok();
        if matches!(state.screen, Screen::Wifi) {
            draw_wifi_overlay(
                terminal.backend_mut().display_mut(),
                state.wifi_client_connected,
            );
        }
        terminal.backend_mut().display_mut().flush().await.ok();
    }
}

fn render_info(area: Rect, buf: &mut Buffer) {
    let title = Line::from("config");
    let block = Block::new().title(title.centered());

    let mhz = LORA_FREQUENCY_HZ / 1_000_000;
    let frac = (LORA_FREQUENCY_HZ % 1_000_000) / 1_000;

    let info_text = vec![
        format!("Freq: {}.{:03} MHz", mhz, frac).into(),
        format!("Preamble len: {}", LORA_PREAMBLE_LEN).into(),
        format!("Sync word: {}", LORA_SYNCWORD).into(),
    ];

    Paragraph::new(info_text)
        .left_aligned()
        .block(block)
        .render(area, buf);
}

struct AppView<'a>(&'a AppState);

impl Widget for AppView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = self.0;
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let header = layout[0];
        let body = layout[1];

        let title = match state.screen {
            Screen::Info => "INFO",
            Screen::Rx => "RX",
            Screen::Tx => "TX",
            Screen::Wifi => "WIFI",
        };

        Paragraph::new(title).left_aligned().render(header, buf);

        let radio_text = match state.radio_state {
            RadioState::Off => "R:OFF",
            RadioState::On(On::Standby) => "R:STBY",
            RadioState::On(On::Busy {
                mode: Mode::Rx { .. },
                ..
            }) => "R:RX",
            RadioState::On(On::Busy {
                mode: Mode::Tx { .. },
                ..
            }) => "R:TX",
        };
        Paragraph::new(radio_text).centered().render(header, buf);

        let battery_text = state
            .battery_level
            .map(|l| {
                let volts = l / 1_000;
                let frac = l % 1_000;
                format!("{}.{:03}", volts, frac)
            })
            .unwrap_or_else(|| "--".to_string());
        Paragraph::new(battery_text)
            .right_aligned()
            .render(header, buf);

        match state.screen {
            Screen::Info => render_info(body, buf),
            Screen::Rx => {
                let (shutdown_pending, last_seen_counter, rssi, snr) =
                    if let RadioState::On(On::Busy {
                        shutting_down,
                        mode:
                            Mode::Rx {
                                last_seen_counter,
                                rssi,
                                snr,
                            },
                    }) = state.radio_state
                    {
                        (shutting_down, last_seen_counter, rssi, snr)
                    } else {
                        (false, None, None, None)
                    };

                let p = if shutdown_pending {
                    Paragraph::new("graceful shutdown").centered()
                } else {
                    Paragraph::new(vec![
                        format!(
                            "last received: {}",
                            last_seen_counter.map_or("n/a".to_string(), |r| r.to_string())
                        )
                        .into(),
                        format!(
                            "rssi: {}",
                            rssi.map_or("n/a".to_string(), |r| r.to_string())
                        )
                        .into(),
                        format!("snr: {}", snr.map_or("n/a".to_string(), |r| r.to_string())).into(),
                    ])
                    .centered()
                };

                p.render(body, buf)
            }

            Screen::Tx => {
                let (shutdown_pending, last_sent_counter) = if let RadioState::On(On::Busy {
                    shutting_down,
                    mode: Mode::Tx { last_sent_counter },
                }) = state.radio_state
                {
                    (shutting_down, last_sent_counter)
                } else {
                    (false, None)
                };

                let p = if shutdown_pending {
                    Paragraph::new("graceful shutdown").centered()
                } else if let Some(last_sent_counter) = last_sent_counter {
                    Paragraph::new(vec![format!("sent: {}", last_sent_counter).into()]).centered()
                } else {
                    Paragraph::new("range test: tx").centered()
                };
                p.render(body, buf)
            }
            Screen::Wifi => {
                let text = if state.wifi_client_connected {
                    let ap_host_port = web::ap_host_port();
                    vec![
                        "PAGE:".into(),
                        format!("{}", ap_host_port.as_str()).into(),
                    ]
                } else {
                    let ap_ssid = web::ap_ssid();
                    let ap_password = web::ap_password();
                    vec![
                        "AP:".into(),
                        format!("{}", ap_ssid.as_str()).into(),
                        "PASS:".into(),
                        format!("{}", ap_password.as_str()).into(),
                    ]
                };

                Paragraph::new(text).left_aligned().render(body, buf)
            }
        }
    }
}

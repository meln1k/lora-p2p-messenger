use embassy_executor::task;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Receiver, Sender},
    mutex::Mutex,
    pubsub::PubSubChannel,
    signal::Signal,
};
use heapless::Vec;

use crate::{
    app::AppState,
    commands::Command,
    events::Event,
    messages::{InboundRadioMessage, MAX_MESSAGE_LEN, MessageLog},
};

pub static APP_EVENTS: Channel<CriticalSectionRawMutex, Event, 8> = Channel::new();
pub static RADIO_COMMANDS: Channel<CriticalSectionRawMutex, Command, 8> = Channel::new();
pub static UI_SIGNAL: Signal<CriticalSectionRawMutex, AppState> = Signal::new();
pub static MESSAGE_LOG: Mutex<CriticalSectionRawMutex, MessageLog> = Mutex::new(MessageLog::new());
pub static OUTBOUND_MESSAGES: Channel<CriticalSectionRawMutex, Vec<u8, MAX_MESSAGE_LEN>, 8> =
    Channel::new();
pub static INBOUND_RADIO_MESSAGES: PubSubChannel<
    CriticalSectionRawMutex,
    InboundRadioMessage,
    16,
    4,
    4,
> = PubSubChannel::new();

#[task]
pub async fn app_task(
    app_events: Receiver<'static, CriticalSectionRawMutex, Event, 8>,
    lora_commands: Sender<'static, CriticalSectionRawMutex, Command, 8>,
    display_signal: &'static Signal<CriticalSectionRawMutex, AppState>,
) {
    let mut state = AppState::new();
    display_signal.signal(state.clone());

    loop {
        let event = app_events.receive().await;
        let commands = state.handle_event(event);
        for c in commands {
            lora_commands.send(c).await;
        }
        display_signal.signal(state.clone());
    }
}

pub async fn log_sent(text: &str) {
    let mut log = MESSAGE_LOG.lock().await;
    log.log_sent(text);
}

pub async fn log_received(payload: &[u8], rssi: i16, snr: i16) {
    let mut log = MESSAGE_LOG.lock().await;
    log.log_received(payload, rssi, snr);
}

use embassy_futures::select::{
    Either::{First, Second},
    select,
};
use lora_p2p_messenger::{
    events::{Button, Event, Press},
    runtime::APP_EVENTS,
};
use esp_hal::gpio::Input;

use crate::button::{ButtonConfig, ButtonEvent::*, DebouncedButton};

#[embassy_executor::task]
pub async fn task(boot_pin: Input<'static>, io3_pin: Input<'static>) {
    let mut boot_btn = DebouncedButton::new(boot_pin, ButtonConfig::default());
    let mut io3_btn = DebouncedButton::new(io3_pin, ButtonConfig::default());

    loop {
        match select(boot_btn.update(), io3_btn.update()).await {
            // if the channel if full, ignore buttons
            First(ShortPress) => APP_EVENTS
                .try_send(Event::ButtonPressed {
                    button: Button::Boot,
                    duration: Press::Short,
                })
                .ok(),
            First(LongPress) => APP_EVENTS
                .try_send(Event::ButtonPressed {
                    button: Button::Boot,
                    duration: Press::Long,
                })
                .ok(),
            Second(ShortPress) => APP_EVENTS
                .try_send(Event::ButtonPressed {
                    button: Button::IO3,
                    duration: Press::Short,
                })
                .ok(),
            Second(LongPress) => APP_EVENTS
                .try_send(Event::ButtonPressed {
                    button: Button::IO3,
                    duration: Press::Long,
                })
                .ok(),
        };
    }
}

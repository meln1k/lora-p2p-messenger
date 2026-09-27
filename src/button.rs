use embassy_time::{Duration, Timer};
use esp_hal::gpio::Input;

/// Button press event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    ShortPress,
    LongPress,
}

/// Configuration for button behavior
#[derive(Debug, Clone, Copy)]
pub struct ButtonConfig {
    /// Debounce delay in milliseconds
    pub debounce_ms: u64,
    /// Long press threshold in milliseconds
    pub long_press_ms: u64,
    /// Polling interval while waiting for button release in milliseconds
    pub poll_interval_ms: u64,
}

impl Default for ButtonConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 20,
            long_press_ms: 800,
            poll_interval_ms: 10,
        }
    }
}

/// Button abstraction that handles debouncing and press event detection
pub struct DebouncedButton<'a> {
    pin: Input<'a>,
    config: ButtonConfig,
    pending_release: bool,
}

impl<'a> DebouncedButton<'a> {
    /// Creates a new button with the given pin and configuration
    pub fn new(pin: Input<'a>, config: ButtonConfig) -> Self {
        Self {
            pin,
            config,
            pending_release: false,
        }
    }

    /// Waits for the next button press event and returns the event type
    ///
    /// This method blocks until a button press is detected, debounced, and classified
    /// as either a short or long press based on the configuration.
    pub async fn update(&mut self) -> ButtonEvent {
        loop {
            if self.pending_release {
                // A long press was reported; wait for release before detecting another press.
                self.pin.wait_for_high().await;
                Timer::after(Duration::from_millis(self.config.debounce_ms)).await;
                self.pending_release = false;
            }

            // Wait for button press (pin goes low, assuming active-low buttons)
            self.pin.wait_for_low().await;

            // Debounce: wait and check if still pressed
            Timer::after(Duration::from_millis(self.config.debounce_ms)).await;

            if self.pin.is_low() {
                // Button is still pressed after debounce, start tracking press duration
                let press_start = embassy_time::Instant::now();

                // Wait for either long press threshold or button release
                let is_long = loop {
                    if self.pin.is_high() {
                        // Button released before long press threshold
                        break false;
                    }

                    let elapsed = embassy_time::Instant::now() - press_start;
                    if elapsed.as_millis() >= self.config.long_press_ms {
                        // Long press detected
                        break true;
                    }

                    // Check again after a short delay
                    Timer::after(Duration::from_millis(self.config.poll_interval_ms)).await;
                };

                if is_long {
                    // Emit long press immediately; defer release wait to next update call.
                    self.pending_release = true;
                    return ButtonEvent::LongPress;
                }
                // Short press - debounce the release
                Timer::after(Duration::from_millis(self.config.debounce_ms)).await;
                return ButtonEvent::ShortPress;
            }
        }
    }
}

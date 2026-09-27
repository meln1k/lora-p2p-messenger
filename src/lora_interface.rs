use embassy_futures::select::{Either, select};
use esp_hal::gpio::{Input, Output};
use lora_phy::{DelayNs, mod_params::RadioError, mod_traits::InterfaceVariant};

/// Custom InterfaceVariant for T-Beam with both DIO0 and DIO1 support
pub struct TBeamSx127xInterfaceVariant {
    reset: Output<'static>,
    dio0: Input<'static>,
    dio1: Input<'static>,
    rf_switch_rx: Option<Output<'static>>,
    rf_switch_tx: Option<Output<'static>>,
}

impl TBeamSx127xInterfaceVariant {
    pub fn new(
        reset: Output<'static>,
        dio0: Input<'static>,
        dio1: Input<'static>,
        rf_switch_rx: Option<Output<'static>>,
        rf_switch_tx: Option<Output<'static>>,
    ) -> Result<Self, RadioError> {
        Ok(Self {
            reset,
            dio0,
            dio1,
            rf_switch_rx,
            rf_switch_tx,
        })
    }
}

impl InterfaceVariant for TBeamSx127xInterfaceVariant {
    async fn reset(&mut self, delay: &mut impl DelayNs) -> Result<(), RadioError> {
        delay.delay_ms(10).await;
        self.reset.set_low();
        delay.delay_ms(10).await;
        self.reset.set_high();
        delay.delay_ms(10).await;
        Ok(())
    }

    async fn await_irq(&mut self) -> Result<(), RadioError> {
        // Wait for either DIO0 (RxDone/TxDone) or DIO1 (RxTimeout)
        match select(self.dio0.wait_for_high(), self.dio1.wait_for_high()).await {
            Either::First(_) => {
                defmt::debug!("IRQ: DIO0 (RxDone/TxDone)");
            }
            Either::Second(_) => {
                defmt::debug!("IRQ: DIO1 (RxTimeout)");
            }
        }
        Ok(())
    }

    async fn wait_on_busy(&mut self) -> Result<(), RadioError> {
        Ok(())
    }

    async fn enable_rf_switch_rx(&mut self) -> Result<(), RadioError> {
        if let Some(pin) = &mut self.rf_switch_tx {
            pin.set_low();
        };
        if let Some(pin) = &mut self.rf_switch_rx {
            pin.set_high();
        }
        Ok(())
    }

    async fn enable_rf_switch_tx(&mut self) -> Result<(), RadioError> {
        if let Some(pin) = &mut self.rf_switch_rx {
            pin.set_low();
        };
        if let Some(pin) = &mut self.rf_switch_tx {
            pin.set_high();
        }
        Ok(())
    }

    async fn disable_rf_switch(&mut self) -> Result<(), RadioError> {
        if let Some(pin) = &mut self.rf_switch_rx {
            pin.set_low();
        };
        if let Some(pin) = &mut self.rf_switch_tx {
            pin.set_low();
        }
        Ok(())
    }
}

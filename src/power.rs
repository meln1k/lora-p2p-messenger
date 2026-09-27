use axp2101_dd::{Axp2101Async, AxpError, AxpInterface, LdoId};
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender, signal::Signal};
use embassy_time::{Duration, Ticker};
use embedded_hal_async::i2c::I2c as AsyncI2c;
use esp_hal::{Async, i2c::master::I2c};
use lora_p2p_messenger::events::Event;

async fn setup_pmu<I2C>(
    i2c: I2C,
) -> Result<Axp2101Async<AxpInterface<I2C>, I2C::Error>, AxpError<I2C::Error>>
where
    I2C: AsyncI2c,
    I2C::Error: core::fmt::Debug,
{
    let mut axp = Axp2101Async::new(i2c);

    // enable the power-off button
    axp.ll
        .power_off_enable()
        .modify_async(|w| {
            w.set_btn_pwroff_en(true); // enable the power-off button
            w.set_btn_pwroff_mode(false); // power-off (not restart!)
        })
        .await?;

    // set power-on/power-off timings
    axp.ll
        .power_on_level()
        .modify_async(|w| {
            w.set_off_level(axp2101_dd::OffLevel::S4);
            w.set_on_level(axp2101_dd::OnLevel::Ms512);
        })
        .await?;

    // battery charging setup
    axp.set_battery_charge_enable(true).await?;
    axp.set_battery_charge_current(axp2101_dd::FastChargeCurrentLimit::Ma500)
        .await?;
    axp.set_battery_charge_voltage(axp2101_dd::ChargeVoltageLimit::V42)
        .await?;

    // set min battery voltage
    axp.ll
        .voff_threshold()
        .modify_async(|w| {
            w.set_voff_thld(axp2101_dd::VoffVoltage::V30);
        })
        .await?;

    // enable charging led and set it to be on when charging
    axp.ll
        .chg_led_control()
        .modify_async(|w| {
            w.set_chgled_en(true);
            w.set_chgled_func(axp2101_dd::ChgledFunction::TypeA);
        })
        .await?;

    // power rail setup

    // gps
    axp.set_ldo_voltage_mv(LdoId::Aldo4, 3300).await?;
    axp.set_ldo_enable(LdoId::Aldo4, true).await?;

    // sd card
    axp.set_ldo_voltage_mv(LdoId::Aldo2, 3300).await?;
    axp.set_ldo_enable(LdoId::Aldo2, true).await?;

    Ok(axp)
}

#[embassy_executor::task]
pub async fn task(
    i2c: I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>,
    app_events: Sender<'static, CriticalSectionRawMutex, Event, 8>,
    pmu_done: &'static Signal<CriticalSectionRawMutex, bool>,
) {
    let mut axp = setup_pmu(i2c).await.expect("pmu setup failed");
    pmu_done.signal(true);

    let mut level = axp.get_battery_voltage_mv().await.ok();
    app_events.send(Event::BatteryLevelUpdate { level }).await;

    let mut ticker = Ticker::every(Duration::from_secs(1));

    loop {
        let new_level = axp.get_battery_voltage_mv().await.ok();
        if new_level != level {
            app_events
                .send(Event::BatteryLevelUpdate { level: new_level })
                .await;
            level = new_level
        }
        ticker.next().await;
    }
}

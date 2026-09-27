//! embassy hello world with SH1106 OLED display
//!
//! This is an example of running the embassy executor with an SH1106 128x64
//! I2C OLED display using separate tasks that communicate via channels.
//!
//! Wiring:
//! - SDA => GPIO8
//! - SCL => GPIO9

#![no_std]
#![no_main]

extern crate alloc;

mod button;
mod display;
mod input;
mod power;
mod radio;

use embassy_embedded_hal::shared_bus::asynch::{i2c::I2cDevice, spi::SpiDevice};
use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex, signal::Signal};
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::{
    Async,
    gpio::{Input, InputConfig, Level, Output, OutputConfig},
    i2c::master::{Config as I2CConfig, I2c},
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
    timer::timg::TimerGroup,
};
use esp_println as _;
use lora_p2p_messenger::{
    runtime::{self, APP_EVENTS, INBOUND_RADIO_MESSAGES, RADIO_COMMANDS, UI_SIGNAL},
    web,
};
use static_cell::StaticCell;

static SPI_BUS: StaticCell<Mutex<CriticalSectionRawMutex, Spi<'static, Async>>> = StaticCell::new();

static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c<'static, Async>>> = StaticCell::new();

esp_bootloader_esp_idf::esp_app_desc!();

static PMU_DONE: Signal<CriticalSectionRawMutex, bool> = Signal::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    // Initialize the peripherals
    let peripherals = esp_hal::init(esp_hal::Config::default());
    defmt::info!("peripherals initialized");
    esp_alloc::heap_allocator!(size: 128 * 1024);

    // Start the scheduler with the timer group and software interrupt peripheral.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    defmt::info!("timer group initialized");
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);
    defmt::info!("esp_rtos started");

    // buttons
    let boot_input = Input::new(peripherals.GPIO0, InputConfig::default());
    let io3_input = Input::new(peripherals.GPIO3, InputConfig::default());
    let wifi = peripherals.WIFI;

    // lora spi setup
    let lora_sclk = peripherals.GPIO12;
    let lora_mosi = peripherals.GPIO11;
    let lora_miso = peripherals.GPIO13;
    let lora_cs = peripherals.GPIO1;
    // silence the sd card
    let mut sd_cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    sd_cs.set_high();

    // lora control pins
    let lora_rst = Output::new(peripherals.GPIO18, Level::High, OutputConfig::default());
    let ldo_en = Output::new(peripherals.GPIO16, Level::Low, OutputConfig::default());
    let ctl_lna = Output::new(peripherals.GPIO39, Level::Low, OutputConfig::default());
    let lora_dio0 = Input::new(peripherals.GPIO14, InputConfig::default());
    let lora_dio1 = Input::new(peripherals.GPIO21, InputConfig::default());

    // Set up I2C on GPIO8 (SDA) and GPIO9 (SCL) at 400 kHz
    let i2c = I2c::new(
        peripherals.I2C0,
        I2CConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .unwrap()
    .with_sda(peripherals.GPIO8)
    .with_scl(peripherals.GPIO9)
    .into_async();
    defmt::info!("i2c initialized");

    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));

    spawner.spawn(power::task(I2cDevice::new(i2c_bus), APP_EVENTS.sender(), &PMU_DONE).unwrap());
    PMU_DONE.wait().await;
    defmt::info!("power setup complete");

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_khz(100))
            .with_mode(Mode::_0),
    )
    .unwrap()
    .with_sck(lora_sclk)
    .with_mosi(lora_mosi)
    .with_miso(lora_miso)
    .into_async();

    let spi_bus = SPI_BUS.init(Mutex::new(spi));

    // Create CS pin as Output and wrap SpiBus into SpiDevice
    let lora_cs = Output::new(lora_cs, Level::High, OutputConfig::default());
    let lora_spi_device = SpiDevice::new(spi_bus, lora_cs);

    // spawn the display task for user feedback
    defmt::info!("spawning app task");
    spawner.spawn(
        runtime::app_task(APP_EVENTS.receiver(), RADIO_COMMANDS.sender(), &UI_SIGNAL).unwrap(),
    );
    defmt::info!("app task spawned");

    // setup wifi
    defmt::info!("spawning wifi tasks");
    if web::spawn_wifi_tasks(&spawner, wifi, APP_EVENTS.sender()).is_err() {
        defmt::warn!("failed to spawn wifi tasks");
    }
    defmt::info!("wifi tasks spawned");

    defmt::info!("spawning input task");
    spawner.spawn(input::task(boot_input, io3_input).unwrap());
    defmt::info!("input task spawned");
    defmt::info!("spawning display task");
    spawner.spawn(display::display_task(I2cDevice::new(i2c_bus), &UI_SIGNAL).unwrap());
    defmt::info!("display task spawned");
    defmt::info!("spawning radio task");
    spawner.spawn(
        radio::task(
            lora_rst,
            ldo_en,
            ctl_lna,
            lora_dio0,
            lora_dio1,
            lora_spi_device,
            RADIO_COMMANDS.receiver(),
            INBOUND_RADIO_MESSAGES.dyn_immediate_publisher(),
            APP_EVENTS.sender(),
        )
        .unwrap(),
    );
    defmt::info!("radio task spawned");

    loop {
        Timer::after_secs(10).await;
    }
}

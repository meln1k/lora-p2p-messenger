#[derive(Clone, Copy, Debug)]
pub enum Button {
    IO3,
    Boot,
}

#[derive(Clone, Copy, Debug)]
pub enum Press {
    Short,
    Long,
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    ButtonPressed { button: Button, duration: Press },
    WifiClientConnectionChanged { connected: bool },
    RadioEnabled,
    RadioDisabled,
    TxModeOn,
    TxDone { counter: u16 },
    RxModeOn,
    RxSuccess { counter: u16, rssi: i16, snr: i16 },
    RadioStandby,
    BatteryLevelUpdate { level: Option<u16> },
}

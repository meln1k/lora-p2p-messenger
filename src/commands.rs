#[derive(Debug, Clone, Copy)]
pub enum Command {
    EnableRadio,
    DisableRadio,
    StartTxMode,
    StopTxMode,
    StartRxMode,
    StopRxMode,
    StartMessagingMode,
}

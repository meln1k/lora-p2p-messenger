use heapless::Vec;

use crate::{
    commands::Command,
    events::{Button, Event, Press},
};

#[derive(Clone, Debug)]
pub struct AppState {
    pub screen: Screen,
    pub wifi_client_connected: bool,
    pub battery_level: Option<u16>,
    pub radio_state: RadioState,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            screen: Screen::Wifi,
            wifi_client_connected: false,
            battery_level: None,
            radio_state: RadioState::Off,
        }
    }

    pub fn handle_event(&mut self, event: Event) -> Vec<Command, 4> {
        match event {
            Event::ButtonPressed {
                button: Button::IO3,
                duration: Press::Short,
            } => {
                self.screen = match self.screen {
                    Screen::Info => Screen::Rx,
                    Screen::Rx => Screen::Tx,
                    Screen::Tx => Screen::Wifi,
                    Screen::Wifi => Screen::Info,
                };
                Vec::new()
            }
            Event::ButtonPressed {
                button: Button::IO3,
                duration: Press::Long,
            } => match self.screen {
                Screen::Tx => match &mut self.radio_state {
                    RadioState::On(On::Standby) => Vec::from_array([Command::StartTxMode]),
                    RadioState::On(On::Busy {
                        shutting_down: true,
                        ..
                    }) => Vec::from_array([Command::DisableRadio]),
                    RadioState::On(On::Busy {
                        shutting_down,
                        mode,
                    }) => match mode {
                        Mode::Rx { .. } => Vec::new(),
                        Mode::Tx { .. } => {
                            *shutting_down = true;
                            Vec::from_array([Command::StopTxMode])
                        }
                    },
                    RadioState::Off => {
                        Vec::from_array([Command::EnableRadio, Command::StartTxMode])
                    }
                },
                Screen::Rx => match &mut self.radio_state {
                    RadioState::On(On::Standby) => Vec::from_array([Command::StartRxMode]),
                    RadioState::On(On::Busy {
                        shutting_down: true,
                        ..
                    }) => Vec::from_array([Command::DisableRadio]),
                    RadioState::On(On::Busy {
                        shutting_down,
                        mode,
                    }) => match mode {
                        Mode::Rx { .. } => {
                            *shutting_down = true;
                            Vec::from_array([Command::StopRxMode])
                        }
                        Mode::Tx { .. } => Vec::new(),
                    },
                    RadioState::Off => {
                        Vec::from_array([Command::EnableRadio, Command::StartRxMode])
                    }
                },
                Screen::Info => Vec::new(),
                Screen::Wifi => match &mut self.radio_state {
                    RadioState::On(_) => Vec::from_array([Command::StartMessagingMode]),
                    RadioState::Off => {
                        Vec::from_array([Command::EnableRadio, Command::StartMessagingMode])
                    }
                },
            },
            // ignore the boot button for now
            Event::ButtonPressed {
                button: Button::Boot,
                ..
            } => Vec::new(),
            Event::WifiClientConnectionChanged { connected } => {
                self.wifi_client_connected = connected;
                Vec::new()
            }
            Event::RadioEnabled => {
                self.radio_state = RadioState::On(On::Standby);
                Vec::new()
            }
            Event::RadioDisabled => {
                self.radio_state = RadioState::Off;
                Vec::new()
            }
            Event::RxModeOn => {
                self.radio_state = RadioState::On(On::Busy {
                    shutting_down: false,
                    mode: Mode::Rx {
                        last_seen_counter: None,
                        rssi: None,
                        snr: None,
                    },
                });
                Vec::new()
            }
            Event::TxModeOn => {
                self.radio_state = RadioState::On(On::Busy {
                    shutting_down: false,
                    mode: Mode::Tx {
                        last_sent_counter: None,
                    },
                });
                Vec::new()
            }
            Event::RxSuccess { counter, rssi, snr } => {
                if let RadioState::On(On::Busy {
                    mode: mode @ Mode::Rx { .. },
                    ..
                }) = &mut self.radio_state
                {
                    *mode = Mode::Rx {
                        last_seen_counter: Some(counter),
                        rssi: Some(rssi),
                        snr: Some(snr),
                    };
                };
                Vec::new()
            }
            Event::TxDone { counter } => {
                if let RadioState::On(On::Busy {
                    mode: mode @ Mode::Tx { .. },
                    ..
                }) = &mut self.radio_state
                {
                    *mode = Mode::Tx {
                        last_sent_counter: Some(counter),
                    }
                };
                Vec::new()
            }
            Event::RadioStandby => {
                self.radio_state = RadioState::On(On::Standby);
                Vec::new()
            }
            Event::BatteryLevelUpdate { level } => {
                self.battery_level = level;
                Vec::new()
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum Screen {
    Info,
    Rx,
    Tx,
    #[default]
    Wifi,
}

#[derive(Clone, Debug)]
pub enum RadioState {
    Off,
    On(On),
}

#[derive(Clone, Debug)]
pub enum On {
    Standby,
    Busy { shutting_down: bool, mode: Mode },
}

#[derive(Clone, Debug)]
pub enum Mode {
    Rx {
        last_seen_counter: Option<u16>,
        rssi: Option<i16>,
        snr: Option<i16>,
    },
    Tx {
        last_sent_counter: Option<u16>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io3_short() -> Event {
        Event::ButtonPressed {
            button: Button::IO3,
            duration: Press::Short,
        }
    }

    fn io3_long() -> Event {
        Event::ButtonPressed {
            button: Button::IO3,
            duration: Press::Long,
        }
    }

    fn boot_short() -> Event {
        Event::ButtonPressed {
            button: Button::Boot,
            duration: Press::Short,
        }
    }

    fn cmd_tag(cmd: Command) -> u8 {
        match cmd {
            Command::EnableRadio => 1,
            Command::DisableRadio => 2,
            Command::StartTxMode => 3,
            Command::StopTxMode => 4,
            Command::StartRxMode => 5,
            Command::StopRxMode => 6,
            Command::StartMessagingMode => 7,
        }
    }

    fn assert_commands(commands: &Vec<Command, 4>, expected: &[Command]) {
        assert_eq!(commands.len(), expected.len());
        for (idx, expected_cmd) in expected.iter().enumerate() {
            assert_eq!(cmd_tag(commands[idx]), cmd_tag(*expected_cmd));
        }
    }

    #[test]
    fn short_press_cycles_screens_without_commands() {
        let mut state = AppState::new();
        assert!(matches!(state.screen, Screen::Wifi));
        assert_commands(&state.handle_event(io3_short()), &[]);
        assert!(matches!(state.screen, Screen::Info));
        assert_commands(&state.handle_event(io3_short()), &[]);
        assert!(matches!(state.screen, Screen::Rx));
        assert_commands(&state.handle_event(io3_short()), &[]);
        assert!(matches!(state.screen, Screen::Tx));
        assert_commands(&state.handle_event(io3_short()), &[]);
        assert!(matches!(state.screen, Screen::Wifi));
    }

    #[test]
    fn boot_button_is_ignored() {
        let mut state = AppState::new();
        state.screen = Screen::Tx;
        state.radio_state = RadioState::On(On::Standby);

        let commands = state.handle_event(boot_short());
        assert_commands(&commands, &[]);
        assert!(matches!(state.screen, Screen::Tx));
        assert!(matches!(state.radio_state, RadioState::On(On::Standby)));
    }

    #[test]
    fn long_press_in_tx_screen_covers_all_radio_states() {
        let mut state = AppState::new();
        state.screen = Screen::Tx;
        state.radio_state = RadioState::Off;
        assert_commands(
            &state.handle_event(io3_long()),
            &[Command::EnableRadio, Command::StartTxMode],
        );

        state.radio_state = RadioState::On(On::Standby);
        assert_commands(&state.handle_event(io3_long()), &[Command::StartTxMode]);

        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Rx {
                last_seen_counter: Some(11),
                rssi: Some(-88),
                snr: Some(9),
            },
        });
        assert_commands(&state.handle_event(io3_long()), &[]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                shutting_down: false,
                mode: Mode::Rx { .. }
            })
        ));

        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Tx {
                last_sent_counter: Some(22),
            },
        });
        assert_commands(&state.handle_event(io3_long()), &[Command::StopTxMode]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                shutting_down: true,
                mode: Mode::Tx { .. }
            })
        ));

        assert_commands(&state.handle_event(io3_long()), &[Command::DisableRadio]);
    }

    #[test]
    fn long_press_in_rx_and_wifi_and_info_screens() {
        let mut state = AppState::new();

        state.screen = Screen::Rx;
        state.radio_state = RadioState::Off;
        assert_commands(
            &state.handle_event(io3_long()),
            &[Command::EnableRadio, Command::StartRxMode],
        );

        state.radio_state = RadioState::On(On::Standby);
        assert_commands(&state.handle_event(io3_long()), &[Command::StartRxMode]);
        assert!(matches!(state.radio_state, RadioState::On(On::Standby)));

        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Tx {
                last_sent_counter: Some(5),
            },
        });
        assert_commands(&state.handle_event(io3_long()), &[]);

        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Rx {
                last_seen_counter: Some(2),
                rssi: Some(-77),
                snr: Some(3),
            },
        });
        assert_commands(&state.handle_event(io3_long()), &[Command::StopRxMode]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Rx {
                    last_seen_counter: Some(2),
                    rssi: Some(-77),
                    snr: Some(3)
                },
                shutting_down: true
            })
        ));
        assert_commands(&state.handle_event(io3_long()), &[Command::DisableRadio]);

        state.screen = Screen::Wifi;
        state.radio_state = RadioState::Off;
        assert_commands(
            &state.handle_event(io3_long()),
            &[Command::EnableRadio, Command::StartMessagingMode],
        );

        state.radio_state = RadioState::On(On::Standby);
        assert_commands(
            &state.handle_event(io3_long()),
            &[Command::StartMessagingMode],
        );

        state.screen = Screen::Info;
        assert_commands(&state.handle_event(io3_long()), &[]);
    }

    #[test]
    fn mode_and_telemetry_events_update_only_expected_state() {
        let mut state = AppState::new();

        assert_commands(&state.handle_event(Event::RadioEnabled), &[]);
        assert!(matches!(state.radio_state, RadioState::On(On::Standby)));

        assert_commands(&state.handle_event(Event::RxModeOn), &[]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                shutting_down: false,
                mode: Mode::Rx {
                    last_seen_counter: None,
                    rssi: None,
                    snr: None,
                }
            })
        ));

        assert_commands(
            &state.handle_event(Event::RxSuccess {
                counter: 7,
                rssi: -90,
                snr: 10,
            }),
            &[],
        );
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Rx {
                    last_seen_counter: Some(7),
                    rssi: Some(-90),
                    snr: Some(10),
                },
                ..
            })
        ));

        assert_commands(&state.handle_event(Event::TxModeOn), &[]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Tx {
                    last_sent_counter: None
                },
                shutting_down: false
            })
        ));

        assert_commands(&state.handle_event(Event::TxDone { counter: 99 }), &[]);
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Tx {
                    last_sent_counter: Some(99)
                },
                ..
            })
        ));
    }

    #[test]
    fn mismatched_telemetry_events_do_not_mutate_mode_payload() {
        let mut state = AppState::new();
        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Tx {
                last_sent_counter: Some(42),
            },
        });
        let _ = state.handle_event(Event::RxSuccess {
            counter: 3,
            rssi: -70,
            snr: 4,
        });
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Tx {
                    last_sent_counter: Some(42)
                },
                shutting_down: false
            })
        ));

        state.radio_state = RadioState::On(On::Busy {
            shutting_down: false,
            mode: Mode::Rx {
                last_seen_counter: Some(1),
                rssi: Some(-80),
                snr: Some(2),
            },
        });
        let _ = state.handle_event(Event::TxDone { counter: 55 });
        assert!(matches!(
            state.radio_state,
            RadioState::On(On::Busy {
                mode: Mode::Rx {
                    last_seen_counter: Some(1),
                    rssi: Some(-80),
                    snr: Some(2),
                },
                shutting_down: false
            })
        ));
    }

    #[test]
    fn radio_standby_disable_and_battery_update_are_deterministic() {
        let mut state = AppState::new();
        state.radio_state = RadioState::Off;
        let _ = state.handle_event(Event::RadioStandby);
        assert!(matches!(state.radio_state, RadioState::On(On::Standby)));

        let _ = state.handle_event(Event::RadioDisabled);
        assert!(matches!(state.radio_state, RadioState::Off));

        let _ = state.handle_event(Event::BatteryLevelUpdate { level: Some(3721) });
        assert_eq!(state.battery_level, Some(3721));
        let _ = state.handle_event(Event::BatteryLevelUpdate { level: None });
        assert_eq!(state.battery_level, None);
    }

    #[test]
    fn wifi_client_connection_event_updates_connection_state() {
        let mut state = AppState::new();
        assert!(!state.wifi_client_connected);

        let _ = state.handle_event(Event::WifiClientConnectionChanged { connected: true });
        assert!(state.wifi_client_connected);

        let _ = state.handle_event(Event::WifiClientConnectionChanged { connected: false });
        assert!(!state.wifi_client_connected);
    }
}

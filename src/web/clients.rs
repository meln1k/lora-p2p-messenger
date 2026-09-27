use heapless::Vec;

pub(super) const MAX_CLIENTS: usize = 4;

/// Tracks distinct AP clients in inline storage inside the Embassy task.
/// No heap allocation is needed, and one disconnect cannot hide another client.
#[derive(Default)]
pub(super) struct ConnectedClients {
    macs: Vec<[u8; 6], MAX_CLIENTS>,
}

impl ConnectedClients {
    pub fn join(&mut self, mac: [u8; 6]) -> Result<(), [u8; 6]> {
        if self.macs.contains(&mac) {
            return Ok(());
        }
        self.macs.push(mac)
    }

    pub fn leave(&mut self, mac: [u8; 6]) {
        if let Some(index) = self.macs.iter().position(|existing| *existing == mac) {
            self.macs.swap_remove(index);
        }
    }

    pub fn any_connected(&self) -> bool {
        !self.macs.is_empty()
    }

    pub fn clear(&mut self) {
        self.macs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: [u8; 6] = [1, 2, 3, 4, 5, 6];
    const LAPTOP: [u8; 6] = [6, 5, 4, 3, 2, 1];

    #[test]
    fn leaving_one_of_two_clients_keeps_ap_connected() {
        let mut clients = ConnectedClients::default();
        clients.join(PHONE).unwrap();
        clients.join(LAPTOP).unwrap();
        clients.leave(PHONE);
        assert!(clients.any_connected());
        clients.leave(LAPTOP);
        assert!(!clients.any_connected());
    }

    #[test]
    fn duplicate_and_unknown_events_do_not_corrupt_membership() {
        let mut clients = ConnectedClients::default();
        clients.join(PHONE).unwrap();
        clients.join(PHONE).unwrap();
        clients.leave(LAPTOP);
        assert!(clients.any_connected());
        clients.leave(PHONE);
        assert!(!clients.any_connected());
        clients.leave(PHONE);
        assert!(!clients.any_connected());
    }

    #[test]
    fn ap_stop_clears_clients_before_restart() {
        let mut clients = ConnectedClients::default();
        clients.join(PHONE).unwrap();
        clients.join(LAPTOP).unwrap();
        clients.clear();
        assert!(!clients.any_connected());
        clients.join(PHONE).unwrap();
        clients.leave(PHONE);
        assert!(!clients.any_connected());
    }

    #[test]
    fn capacity_limit_preserves_clients_and_reuses_freed_slots() {
        let mut clients = ConnectedClients::default();
        for id in 0..MAX_CLIENTS as u8 {
            clients.join([id; 6]).unwrap();
        }
        // A repeated event does not consume a slot, even when full.
        clients.join([0; 6]).unwrap();
        assert_eq!(
            clients.join([MAX_CLIENTS as u8; 6]),
            Err([MAX_CLIENTS as u8; 6])
        );
        clients.leave([1; 6]);
        clients.join([MAX_CLIENTS as u8; 6]).unwrap();
        for id in 0..MAX_CLIENTS as u8 {
            clients.leave([id; 6]);
        }
        assert!(clients.any_connected());
        clients.leave([MAX_CLIENTS as u8; 6]);
        assert!(!clients.any_connected());
    }
}

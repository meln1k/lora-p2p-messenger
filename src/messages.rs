use core::str;

use heapless::{String, Vec};

pub const MAX_MESSAGE_LEN: usize = 64;
pub const MAX_LOG_ENTRIES: usize = 24;

#[derive(Clone, Copy, Debug)]
pub enum MessageDirection {
    Tx,
    Rx,
}

#[derive(Clone, Debug)]
pub struct MessageEntry {
    pub direction: MessageDirection,
    pub text: String<MAX_MESSAGE_LEN>,
    pub rssi: Option<i16>,
    pub snr: Option<i16>,
}

#[derive(Clone, Debug)]
pub struct InboundRadioMessage {
    pub payload: Vec<u8, MAX_MESSAGE_LEN>,
    pub rssi: i16,
    pub snr: i16,
}

#[derive(Debug)]
pub struct MessageLog {
    entries: Vec<MessageEntry, MAX_LOG_ENTRIES>,
}

impl MessageLog {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, entry: MessageEntry) {
        if self.entries.is_full() {
            self.entries.remove(0);
        }
        let _ = self.entries.push(entry);
    }

    pub fn snapshot(&self) -> Vec<MessageEntry, MAX_LOG_ENTRIES> {
        self.entries.clone()
    }

    pub fn log_sent(&mut self, text: &str) {
        let mut msg = String::<MAX_MESSAGE_LEN>::new();
        push_truncated(&mut msg, text);
        self.push(MessageEntry {
            direction: MessageDirection::Tx,
            text: msg,
            rssi: None,
            snr: None,
        });
    }

    pub fn log_received(&mut self, payload: &[u8], rssi: i16, snr: i16) {
        let text = str::from_utf8(payload).unwrap_or("<invalid utf8>");
        let mut msg = String::<MAX_MESSAGE_LEN>::new();
        push_truncated(&mut msg, text);
        self.push(MessageEntry {
            direction: MessageDirection::Rx,
            text: msg,
            rssi: Some(rssi),
            snr: Some(snr),
        });
    }
}

fn push_truncated(dst: &mut String<MAX_MESSAGE_LEN>, src: &str) {
    for ch in src.chars() {
        if dst.push(ch).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx_entry(text: &str) -> MessageEntry {
        let mut msg = String::<MAX_MESSAGE_LEN>::new();
        let _ = msg.push_str(text);
        MessageEntry {
            direction: MessageDirection::Tx,
            text: msg,
            rssi: None,
            snr: None,
        }
    }

    #[test]
    fn fifo_eviction_keeps_exact_latest_window() {
        let mut log = MessageLog::new();

        for i in 0..(MAX_LOG_ENTRIES + 5) {
            let mut text = String::<MAX_MESSAGE_LEN>::new();
            let _ = text.push_str("m");
            let _ = text.push_str(i.to_string().as_str());
            log.push(tx_entry(text.as_str()));
        }

        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), MAX_LOG_ENTRIES);
        assert_eq!(snapshot[0].text.as_str(), "m5");
        assert_eq!(snapshot[MAX_LOG_ENTRIES - 1].text.as_str(), "m28");
    }

    #[test]
    fn snapshot_is_independent_copy() {
        let mut log = MessageLog::new();
        log.push(tx_entry("first"));
        let first = log.snapshot();

        log.push(tx_entry("second"));
        let second = log.snapshot();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].text.as_str(), "first");
        assert_eq!(second.len(), 2);
        assert_eq!(second[1].text.as_str(), "second");
    }

    #[test]
    fn log_sent_sets_tx_metadata_and_truncates_to_capacity() {
        let mut log = MessageLog::new();
        let too_long = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789++++";
        log.log_sent(too_long);

        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(snapshot[0].direction, MessageDirection::Tx));
        assert_eq!(snapshot[0].rssi, None);
        assert_eq!(snapshot[0].snr, None);
        assert_eq!(snapshot[0].text.len(), MAX_MESSAGE_LEN);
        assert_eq!(
            snapshot[0].text.as_str(),
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789++"
        );
    }

    #[test]
    fn log_received_sets_rx_metadata_for_valid_utf8() {
        let mut log = MessageLog::new();
        log.log_received(b"hello", -67, 9);

        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(matches!(snapshot[0].direction, MessageDirection::Rx));
        assert_eq!(snapshot[0].text.as_str(), "hello");
        assert_eq!(snapshot[0].rssi, Some(-67));
        assert_eq!(snapshot[0].snr, Some(9));
    }

    #[test]
    fn log_received_invalid_utf8_is_marked() {
        let mut log = MessageLog::new();
        log.log_received(&[0xff, 0xfe], -70, 8);

        let snapshot = log.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].text.as_str(), "<invalid utf8>");
        assert_eq!(snapshot[0].rssi, Some(-70));
        assert_eq!(snapshot[0].snr, Some(8));
    }
}

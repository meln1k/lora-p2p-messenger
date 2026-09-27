use core::cell::RefCell;

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use heapless::String;

#[cfg(any(target_arch = "xtensa", test))]
mod clients;
#[cfg(target_arch = "xtensa")]
mod hw;
#[cfg(target_arch = "xtensa")]
pub(crate) mod network;
pub(crate) mod server;
#[cfg(target_arch = "xtensa")]
pub use hw::spawn_wifi_tasks;

pub(crate) const AP_IP: &str = "192.168.2.1";
pub(crate) const AP_PORT: u16 = 8080;
const AP_PASSWORD_LEN: usize = 12;
const AP_PASSWORD_CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

struct ApCredentials {
    ssid: String<8>,
    password: String<32>,
    qr_payload: String<96>,
}

static AP_CREDENTIALS: Mutex<CriticalSectionRawMutex, RefCell<ApCredentials>> =
    Mutex::new(RefCell::new(ApCredentials {
        ssid: String::new(),
        password: String::new(),
        qr_payload: String::new(),
    }));

pub fn ap_ssid() -> String<8> {
    AP_CREDENTIALS.lock(|cell| cell.borrow().ssid.clone())
}

pub fn ap_host_port() -> String<32> {
    let mut out = String::<32>::new();
    let _ = out.push_str(AP_IP);
    let _ = core::fmt::write(&mut out, format_args!(":{}", AP_PORT));
    out
}

pub fn ap_url() -> String<48> {
    let mut out = String::<48>::new();
    let _ = out.push_str("http://");
    let host_port = ap_host_port();
    let _ = out.push_str(host_port.as_str());
    let _ = out.push('/');
    out
}

pub fn ap_password() -> String<32> {
    AP_CREDENTIALS.lock(|cell| cell.borrow().password.clone())
}

pub fn ap_wifi_qr_payload() -> String<96> {
    AP_CREDENTIALS.lock(|cell| cell.borrow().qr_payload.clone())
}

pub(super) fn derive_ap_ssid_from(mac: [u8; 6]) -> String<8> {
    let mut out = String::<8>::new();
    let _ = core::fmt::write(&mut out, format_args!("{:08x}", fnv1a32(mac)));
    out
}

pub(super) fn derive_ap_password_from(mac: [u8; 6], seed: &str) -> String<32> {
    let mut out = String::<32>::new();
    for i in 0..AP_PASSWORD_LEN {
        let hash = fnv1a32_salted(mac, seed.as_bytes(), i as u8);
        let idx = (hash as usize) % AP_PASSWORD_CHARSET.len();
        let _ = out.push(AP_PASSWORD_CHARSET[idx] as char);
    }
    out
}

pub(super) fn fnv1a32(input: [u8; 6]) -> u32 {
    let mut hash: u32 = 0x811c9dc5;
    for byte in input {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub(super) fn fnv1a32_salted(mac: [u8; 6], seed: &[u8], salt: u8) -> u32 {
    let mut hash: u32 = 0x811c9dc5 ^ salt as u32;
    for byte in mac {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    for &byte in seed {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(target_arch = "xtensa")]
pub(super) fn set_ap_credentials(ssid: String<8>, password: String<32>, qr_payload: String<96>) {
    AP_CREDENTIALS.lock(|cell| {
        *cell.borrow_mut() = ApCredentials {
            ssid,
            password,
            qr_payload,
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MAC: [u8; 6] = [0x24, 0x6f, 0x28, 0xaa, 0xbb, 0xcc];

    #[test]
    fn ap_ssid_is_stable_hex_from_mac() {
        let ssid_a = derive_ap_ssid_from(TEST_MAC);
        let ssid_b = derive_ap_ssid_from(TEST_MAC);
        assert_eq!(ssid_a, ssid_b);
        assert_eq!(ssid_a.len(), 8);
        assert!(ssid_a.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn ap_password_is_deterministic_and_charset_bounded() {
        let pw1 = derive_ap_password_from(TEST_MAC, "seed-123");
        let pw2 = derive_ap_password_from(TEST_MAC, "seed-123");
        assert_eq!(pw1, pw2);
        assert_eq!(pw1.len(), AP_PASSWORD_LEN);
        assert!(
            pw1.as_str()
                .bytes()
                .all(|b| AP_PASSWORD_CHARSET.contains(&b))
        );
    }

    #[test]
    fn ap_password_changes_when_seed_changes() {
        let pw1 = derive_ap_password_from(TEST_MAC, "seed-123");
        let pw2 = derive_ap_password_from(TEST_MAC, "seed-456");
        assert_ne!(pw1, pw2);
    }

    #[test]
    fn fnv1a32_is_sensitive_to_input() {
        let a = fnv1a32([1, 2, 3, 4, 5, 6]);
        let b = fnv1a32([1, 2, 3, 4, 5, 7]);
        assert_ne!(a, b);
    }
}

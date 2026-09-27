use core::{net::Ipv4Addr, str::FromStr};

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_net::{Ipv4Cidr, StackResources, StaticConfigV4};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use esp_hal::{efuse::Efuse, rng::Rng};
use heapless::String;

use super::{
    AP_IP,
    derive_ap_password_from,
    derive_ap_ssid_from,
    network,
    server,
    set_ap_credentials,
};
use crate::events::Event;

const DEFAULT_AP_PASSWORD_SEED: &str = "tbeam-dev-seed";

macro_rules! mk_static {
    ($t:ty,$val:expr) => {{
        static STATIC_CELL: static_cell::StaticCell<$t> = static_cell::StaticCell::new();
        #[deny(unused_attributes)]
        let x = STATIC_CELL.uninit().write(($val));
        x
    }};
}

pub fn spawn_wifi_tasks(
    spawner: &Spawner,
    wifi: esp_hal::peripherals::WIFI<'static>,
    app_events: Sender<'static, CriticalSectionRawMutex, Event, 8>,
) -> Result<(), ()> {
    info!("web: preparing WiFi AP stack");
    let rng = Rng::new();
    let ssid = derive_ap_ssid();
    let password = derive_ap_password();

    let mut qr_payload = String::<96>::new();
    let _ = qr_payload.push_str("WIFI:T:WPA;S:");
    let _ = qr_payload.push_str(ssid.as_str());
    let _ = qr_payload.push_str(";P:");
    let _ = qr_payload.push_str(password.as_str());
    let _ = qr_payload.push_str(";;");
    set_ap_credentials(ssid.clone(), password.clone(), qr_payload);

    info!(
        "web: AP credentials generated (ssid={}, pass_len={})",
        ssid.as_str(),
        password.len()
    );

    let (controller, interfaces) =
        esp_radio::wifi::new(wifi, Default::default()).map_err(|_| ())?;
    info!("web: WiFi driver initialized");
    let device = interfaces.access_point;

    let gw_ip_addr = Ipv4Addr::from_str(AP_IP).unwrap();
    let config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(gw_ip_addr, 24),
        gateway: Some(gw_ip_addr),
        dns_servers: Default::default(),
    });
    let seed = (rng.random() as u64) << 32 | rng.random() as u64;

    let (stack, runner) = embassy_net::new(
        device,
        config,
        mk_static!(StackResources<5>, StackResources::<5>::new()),
        seed,
    );
    info!("web: embassy-net stack initialized");

    info!("web: spawning WiFi connection task");
    spawner
        .spawn(network::connection_task(
            controller, ssid, password, app_events,
        ))
        .map_err(|_| ())?;
    info!("web: spawning net runner task");
    spawner.spawn(network::net_task(runner)).map_err(|_| ())?;
    info!("web: spawning DHCP server task");
    spawner.spawn(network::dhcp_task(stack)).map_err(|_| ())?;
    info!("web: spawning HTTP server task");
    spawner
        .spawn(server::http_server_task(stack))
        .map_err(|_| ())?;
    info!("web: all WiFi tasks spawned");

    Ok(())
}

fn derive_ap_ssid() -> String<8> {
    derive_ap_ssid_from(Efuse::mac_address())
}

fn derive_ap_password() -> String<32> {
    if option_env!("AP_PASSWORD_SEED").is_none() {
        warn!("web: AP_PASSWORD_SEED unset, using default seed");
    }

    derive_ap_password_from(Efuse::mac_address(), ap_password_seed())
}

fn ap_password_seed() -> &'static str {
    option_env!("AP_PASSWORD_SEED").unwrap_or(DEFAULT_AP_PASSWORD_SEED)
}

use core::{
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
};

use defmt::{debug, info, warn};
use edge_dhcp::{
    io::{self, DEFAULT_SERVER_PORT},
    server::{Server, ServerOptions},
};
use edge_nal::UdpBind;
use edge_nal_embassy::{Udp, UdpBuffers};
use embassy_net::{Runner, Stack};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Sender};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    Config, Interface, WifiController,
    event::{EventInfo, MessageResult},
};

use super::{AP_IP, clients::ConnectedClients};
use crate::events::Event;

#[embassy_executor::task]
pub(crate) async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

#[embassy_executor::task]
pub(crate) async fn connection_task(
    mut controller: WifiController<'static>,
    ap_config: Config,
    app_events: Sender<'static, CriticalSectionRawMutex, Event, 8>,
) {
    info!("web: connection task started");
    let mut clients = ConnectedClients::default();

    loop {
        // Keep a subscription alive so events are retained while app events are sent.
        {
            let mut events = controller
                .subscribe()
                .expect("Wi-Fi event subscription failed");
            loop {
                let event = match events.next_event().await {
                    MessageResult::Message(event) => event,
                    MessageResult::Lagged(missed) => {
                        // There is no public AP client-list query to rebuild our state.
                        warn!(
                            "web: missed {} Wi-Fi events; client state may be stale",
                            missed
                        );
                        continue;
                    }
                };
                let was_connected = clients.any_connected();
                match event {
                    EventInfo::AccessPointStationConnected { mac, .. } => {
                        if clients.join(mac).is_err() {
                            warn!("web: AP client tracking full; client state may be stale");
                        }
                        info!("web: AP client joined: {=[u8; 6]:x}", mac);
                    }
                    EventInfo::AccessPointStationDisconnected { mac, reason, .. } => {
                        clients.leave(mac);
                        info!("web: AP client left: {=[u8; 6]:x}, reason={}", mac, reason);
                    }
                    EventInfo::AccessPointStop => {
                        warn!("web: AP stop event observed");
                        break;
                    }
                    _ => continue,
                };
                let connected = clients.any_connected();
                if connected != was_connected {
                    app_events
                        .send(Event::WifiClientConnectionChanged { connected })
                        .await;
                }
            }
        }

        if clients.any_connected() {
            clients.clear();
            app_events
                .send(Event::WifiClientConnectionChanged { connected: false })
                .await;
        }
        // set_config replaces the old separate configure/start calls.
        loop {
            Timer::after(Duration::from_secs(1)).await;
            if controller.set_config(&ap_config).is_ok() {
                info!("web: AP restarted");
                break;
            }
            warn!("web: failed to restart AP");
        }
    }
}

#[embassy_executor::task]
pub(crate) async fn dhcp_task(stack: Stack<'static>) {
    info!("web: DHCP task started");
    let ip = Ipv4Addr::from_str(AP_IP).unwrap();
    let mut buf = [0u8; 1500];
    let mut gw_buf = [Ipv4Addr::UNSPECIFIED];
    let buffers = UdpBuffers::<3, 1024, 1024, 8>::new();
    let unbound_socket = Udp::new(stack, &buffers);
    let mut bound_socket = unbound_socket
        .bind(core::net::SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_SERVER_PORT,
        )))
        .await
        .unwrap();

    loop {
        let _ = io::server::run(
            &mut Server::<_, 32>::new_with_et(ip),
            &ServerOptions::new(ip, Some(&mut gw_buf)),
            &mut bound_socket,
            &mut buf,
        )
        .await;
        debug!("web: DHCP server iteration complete");
        Timer::after(Duration::from_millis(200)).await;
    }
}

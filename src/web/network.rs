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
    AuthenticationMethod,
    ModeConfig,
    WifiController,
    WifiDevice,
    WifiEvent,
    ap::AccessPointConfig,
};
use heapless::String;

use super::AP_IP;
use crate::events::Event;

#[embassy_executor::task]
pub(crate) async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
pub(crate) async fn connection_task(
    mut controller: WifiController<'static>,
    ap_ssid: String<8>,
    ap_password: String<32>,
    app_events: Sender<'static, CriticalSectionRawMutex, Event, 8>,
) {
    info!("web: connection task started");
    let mut client_connected = false;

    loop {
        if !matches!(controller.is_started(), Ok(true)) {
            info!("web: AP stopped, configuring and starting AP");
            let ap_cfg = ModeConfig::AccessPoint(
                AccessPointConfig::default()
                    .with_ssid(ap_ssid.as_str().into())
                    .with_auth_method(AuthenticationMethod::Wpa2Personal)
                    .with_password(ap_password.as_str().into()),
            );
            if controller.set_config(&ap_cfg).is_err() {
                warn!("web: failed to set AP config");
            }
            if controller.start_async().await.is_err() {
                warn!("web: failed to start AP");
            } else {
                info!("web: AP start requested");
            }
            if client_connected {
                client_connected = false;
                app_events
                    .send(Event::WifiClientConnectionChanged { connected: false })
                    .await;
            }
        } else {
            debug!("web: waiting for AP station-connect/disconnect or stop event");
            let events = controller
                .wait_for_events(
                    WifiEvent::AccessPointStationConnected
                        | WifiEvent::AccessPointStationDisconnected
                        | WifiEvent::AccessPointStop,
                    true,
                )
                .await;

            if events.contains(WifiEvent::AccessPointStationConnected) {
                info!("web: AP client joined");
                if !client_connected {
                    client_connected = true;
                    app_events
                        .send(Event::WifiClientConnectionChanged { connected: true })
                        .await;
                }
            }
            if events.contains(WifiEvent::AccessPointStationDisconnected) {
                info!("web: AP client left");
                if client_connected {
                    client_connected = false;
                    app_events
                        .send(Event::WifiClientConnectionChanged { connected: false })
                        .await;
                }
            }
            if events.contains(WifiEvent::AccessPointStop) {
                warn!("web: AP stop event observed");
                if client_connected {
                    client_connected = false;
                    app_events
                        .send(Event::WifiClientConnectionChanged { connected: false })
                        .await;
                }
                Timer::after(Duration::from_millis(1000)).await;
            }
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

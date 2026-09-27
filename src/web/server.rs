#![cfg_attr(not(target_arch = "xtensa"), allow(dead_code))]

use core::{cell::RefCell, str};

use defmt::{debug, info, warn};
use embassy_net::{IpListenEndpoint, Stack, tcp::TcpSocket};
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex},
    pubsub::{DynSubscriber, WaitResult},
};
use embassy_time::{Duration, Timer};
use heapless::{String, Vec};

use super::{AP_PORT, ap_password, ap_ssid, ap_url};
use crate::{
    messages::{
        InboundRadioMessage,
        MAX_LOG_ENTRIES,
        MAX_MESSAGE_LEN,
        MessageDirection,
        MessageEntry,
    },
    runtime::{INBOUND_RADIO_MESSAGES, MESSAGE_LOG, OUTBOUND_MESSAGES, log_received, log_sent},
};

const INDEX_HTML: &str = include_str!("index.html");

struct EventState {
    seq: u32,
    last_entry: Option<MessageEntry>,
}

static EVENT_STATE: BlockingMutex<CriticalSectionRawMutex, RefCell<EventState>> =
    BlockingMutex::new(RefCell::new(EventState {
        seq: 0,
        last_entry: None,
    }));

#[derive(Clone, Copy, PartialEq, Eq)]
enum HttpMethod {
    Get,
    Post,
    Other,
}

#[derive(Clone, Copy)]
struct RequestMeta<'a> {
    method: HttpMethod,
    path: &'a str,
    body: &'a [u8],
    last_event_id: Option<u32>,
}

#[embassy_executor::task]
pub(crate) async fn http_server_task(stack: Stack<'static>) {
    info!(
        "web: HTTP server task started at {}:{}",
        super::AP_IP,
        AP_PORT
    );
    let mut inbound_sub = INBOUND_RADIO_MESSAGES
        .dyn_subscriber()
        .expect("inbound pubsub subscriber unavailable");

    let mut rx_buffer = [0; 2048];
    let mut tx_buffer = [0; 6144];
    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(15)));

    loop {
        if !stack.is_link_up() {
            debug!("web: link down; waiting");
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }

        let accepted = socket
            .accept(IpListenEndpoint {
                addr: None,
                port: AP_PORT,
            })
            .await;
        if accepted.is_err() {
            warn!("web: HTTP accept error; aborting socket");
            socket.abort();
            continue;
        }

        handle_one_http_request(&mut socket, &mut inbound_sub).await;
        socket.close();
        Timer::after(Duration::from_millis(20)).await;
        socket.abort();
    }
}

async fn handle_one_http_request(
    socket: &mut TcpSocket<'_>,
    inbound_sub: &mut DynSubscriber<'static, InboundRadioMessage>,
) {
    let mut req = [0u8; 2048];
    let mut total = 0usize;
    let mut header_end = None;
    let mut content_len = 0usize;

    while total < req.len() {
        match socket.read(&mut req[total..]).await {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                if header_end.is_none()
                    && let Some(idx) = find_bytes(&req[..total], b"\r\n\r\n")
                {
                    header_end = Some(idx + 4);
                    content_len = parse_content_length(&req[..idx]).unwrap_or(0);
                }
                if let Some(h_end) = header_end
                    && total >= h_end + content_len
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let h_end = header_end.unwrap_or(total);
    let request = &req[..total];
    let Some(meta) = parse_request_meta(request, h_end) else {
        let _ = write_plain_response(socket, "400 Bad Request", "bad request").await;
        return;
    };

    info!("web: request {} {}", method_name(meta.method), meta.path);

    match (meta.method, meta.path) {
        (HttpMethod::Get, "/") => {
            let _ = write_response(
                socket,
                "200 OK",
                "text/html; charset=utf-8",
                INDEX_HTML.as_bytes(),
            )
            .await;
        }
        (HttpMethod::Get, "/api/info") => {
            let body = render_info_json();
            let _ = write_response(socket, "200 OK", "application/json", body.as_bytes()).await;
        }
        (HttpMethod::Get, "/api/messages") => {
            drain_inbound_messages(inbound_sub).await;
            let snapshot = {
                let log = MESSAGE_LOG.lock().await;
                log.snapshot()
            };
            let body = render_log_json(&snapshot);
            let _ = write_response(socket, "200 OK", "application/json", body.as_bytes()).await;
        }
        (HttpMethod::Get, "/events") => {
            let _ = handle_sse(socket, inbound_sub, meta.last_event_id).await;
        }
        (HttpMethod::Post, "/api/send") => {
            if enqueue_outbound_message(meta.body).await {
                let _ = write_plain_response(socket, "202 Accepted", "ok").await;
            } else {
                let _ = write_plain_response(socket, "400 Bad Request", "invalid or full").await;
            }
        }
        _ => {
            let _ = write_plain_response(socket, "404 Not Found", "not found").await;
        }
    }
}

async fn handle_sse(
    socket: &mut TcpSocket<'_>,
    inbound_sub: &mut DynSubscriber<'static, InboundRadioMessage>,
    last_event_id: Option<u32>,
) -> Result<(), ()> {
    drain_inbound_messages(inbound_sub).await;

    let Some(last_id) = last_event_id else {
        let snapshot = {
            let log = MESSAGE_LOG.lock().await;
            log.snapshot()
        };
        let (seq, _) = event_state();
        let payload = render_sse_snapshot_payload(&snapshot);
        return write_sse_response(socket, seq, payload.as_str()).await;
    };

    for _ in 0..4 {
        let (seq, last_entry) = event_state();
        if seq > last_id {
            if seq == last_id.wrapping_add(1)
                && let Some(entry) = last_entry
            {
                let payload = render_sse_append_payload(&entry);
                return write_sse_response(socket, seq, payload.as_str()).await;
            }

            let snapshot = {
                let log = MESSAGE_LOG.lock().await;
                log.snapshot()
            };
            let payload = render_sse_snapshot_payload(&snapshot);
            return write_sse_response(socket, seq, payload.as_str()).await;
        }

        drain_inbound_messages(inbound_sub).await;
        Timer::after(Duration::from_millis(50)).await;
    }

    let (seq, _) = event_state();
    write_sse_response(socket, seq, "{\"type\":\"noop\"}").await
}

async fn enqueue_outbound_message(body: &[u8]) -> bool {
    let Some(raw_msg) = parse_form_field(body, "msg") else {
        return false;
    };

    if raw_msg.is_empty() {
        return false;
    }

    let mut outbound = Vec::<u8, MAX_MESSAGE_LEN>::new();
    let _ = outbound.extend_from_slice(raw_msg.as_bytes());
    if OUTBOUND_MESSAGES.try_send(outbound).is_err() {
        warn!("web: outbound queue full, message dropped");
        return false;
    }

    let entry = MessageEntry {
        direction: MessageDirection::Tx,
        text: raw_msg,
        rssi: None,
        snr: None,
    };
    log_sent(entry.text.as_str()).await;
    let seq = note_append(&entry);
    debug!("web: log append seq={}", seq);
    true
}

async fn drain_inbound_messages(subscriber: &mut DynSubscriber<'static, InboundRadioMessage>) {
    while let Some(msg) = subscriber.try_next_message() {
        match msg {
            WaitResult::Message(msg) => {
                log_received(msg.payload.as_slice(), msg.rssi, msg.snr).await;
                let entry = inbound_to_entry(msg.payload.as_slice(), msg.rssi, msg.snr);
                let seq = note_append(&entry);
                debug!("web: log append seq={}", seq);
            }
            WaitResult::Lagged(_) => continue,
        }
    }
}

fn inbound_to_entry(payload: &[u8], rssi: i16, snr: i16) -> MessageEntry {
    let text = str::from_utf8(payload).unwrap_or("<invalid utf8>");
    let mut msg = String::<MAX_MESSAGE_LEN>::new();
    push_truncated(&mut msg, text);
    MessageEntry {
        direction: MessageDirection::Rx,
        text: msg,
        rssi: Some(rssi),
        snr: Some(snr),
    }
}

fn push_truncated(dst: &mut String<MAX_MESSAGE_LEN>, src: &str) {
    for ch in src.chars() {
        if dst.push(ch).is_err() {
            break;
        }
    }
}

fn note_append(entry: &MessageEntry) -> u32 {
    EVENT_STATE.lock(|cell| {
        let mut state = cell.borrow_mut();
        state.seq = state.seq.wrapping_add(1);
        state.last_entry = Some(entry.clone());
        state.seq
    })
}

fn event_state() -> (u32, Option<MessageEntry>) {
    EVENT_STATE.lock(|cell| {
        let state = cell.borrow();
        (state.seq, state.last_entry.clone())
    })
}

async fn write_plain_response(
    socket: &mut TcpSocket<'_>,
    status: &str,
    body: &str,
) -> Result<(), ()> {
    write_response(socket, status, "text/plain; charset=utf-8", body.as_bytes()).await
}

async fn write_response(
    socket: &mut TcpSocket<'_>,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), ()> {
    use embedded_io_async::Write;

    let mut header = String::<192>::new();
    let _ = core::fmt::write(
        &mut header,
        format_args!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            status,
            content_type,
            body.len(),
        ),
    );

    socket.write_all(header.as_bytes()).await.map_err(|_| ())?;
    socket.write_all(body).await.map_err(|_| ())?;
    socket.flush().await.map_err(|_| ())?;
    Ok(())
}

async fn write_sse_response(
    socket: &mut TcpSocket<'_>,
    id: u32,
    json_payload: &str,
) -> Result<(), ()> {
    let mut body = String::<4608>::new();
    let _ = core::fmt::write(
        &mut body,
        format_args!("retry: 1000\nid: {}\ndata: {}\n\n", id, json_payload),
    );

    write_response(
        socket,
        "200 OK",
        "text/event-stream; charset=utf-8",
        body.as_bytes(),
    )
    .await
}

fn parse_request_meta(request: &[u8], header_end: usize) -> Option<RequestMeta<'_>> {
    let head = core::str::from_utf8(&request[..header_end]).ok()?;
    let mut lines = head.lines();
    let line = lines.next()?;
    let mut parts = line.split_whitespace();

    let method = parse_method(parts.next()?);
    let path = strip_query(parts.next()?);
    let mut last_event_id = None;

    for hdr in lines {
        let Some((name, value)) = hdr.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("last-event-id") {
            last_event_id = value.trim().parse::<u32>().ok();
        }
    }

    let body = if request.len() >= header_end {
        &request[header_end..]
    } else {
        &[]
    };

    Some(RequestMeta {
        method,
        path,
        body,
        last_event_id,
    })
}

fn parse_method(method: &str) -> HttpMethod {
    match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        _ => HttpMethod::Other,
    }
}

fn method_name(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Other => "OTHER",
    }
}

fn strip_query(path: &str) -> &str {
    path.split_once('?').map(|(left, _)| left).unwrap_or(path)
}

fn render_info_json() -> String<256> {
    let mut out = String::<256>::new();
    let _ = out.push_str("{\"ssid\":\"");
    let _ = json_escape_push(&mut out, ap_ssid().as_str());
    let _ = out.push_str("\",\"password\":\"");
    let _ = json_escape_push(&mut out, ap_password().as_str());
    let _ = out.push_str("\",\"url\":\"");
    let _ = json_escape_push(&mut out, ap_url().as_str());
    let _ = out.push_str("\"}");
    out
}

fn render_log_json(log: &Vec<MessageEntry, MAX_LOG_ENTRIES>) -> String<4096> {
    let mut out = String::<4096>::new();
    let _ = out.push_str("{\"entries\":[");
    append_entries_json(&mut out, log);
    let _ = out.push_str("]}");
    out
}

fn render_sse_snapshot_payload(log: &Vec<MessageEntry, MAX_LOG_ENTRIES>) -> String<4608> {
    let mut out = String::<4608>::new();
    let _ = out.push_str("{\"type\":\"snapshot\",\"entries\":[");
    append_entries_json(&mut out, log);
    let _ = out.push_str("]}");
    out
}

fn render_sse_append_payload(entry: &MessageEntry) -> String<512> {
    let mut out = String::<512>::new();
    let _ = out.push_str("{\"type\":\"append\",\"entry\":");
    append_entry_json(&mut out, entry);
    let _ = out.push('}');
    out
}

fn append_entries_json<const N: usize>(
    dst: &mut String<N>,
    log: &Vec<MessageEntry, MAX_LOG_ENTRIES>,
) {
    let mut first = true;
    for entry in log.iter() {
        if !first {
            let _ = dst.push(',');
        }
        first = false;
        append_entry_json(dst, entry);
    }
}

fn append_entry_json<const N: usize>(dst: &mut String<N>, entry: &MessageEntry) {
    let _ = dst.push_str("{\"direction\":\"");
    match entry.direction {
        MessageDirection::Tx => {
            let _ = dst.push_str("TX");
        }
        MessageDirection::Rx => {
            let _ = dst.push_str("RX");
        }
    }

    let _ = dst.push_str("\",\"text\":\"");
    let _ = json_escape_push(dst, entry.text.as_str());
    let _ = dst.push_str("\",\"rssi\":");
    match entry.rssi {
        Some(v) => {
            let _ = core::fmt::write(dst, format_args!("{}", v));
        }
        None => {
            let _ = dst.push_str("null");
        }
    }
    let _ = dst.push_str(",\"snr\":");
    match entry.snr {
        Some(v) => {
            let _ = core::fmt::write(dst, format_args!("{}", v));
        }
        None => {
            let _ = dst.push_str("null");
        }
    }
    let _ = dst.push('}');
}

fn json_escape_push<const N: usize>(dst: &mut String<N>, src: &str) -> Result<(), ()> {
    for ch in src.chars() {
        match ch {
            '"' => dst.push_str("\\\"").map_err(|_| ())?,
            '\\' => dst.push_str("\\\\").map_err(|_| ())?,
            '\n' => dst.push_str("\\n").map_err(|_| ())?,
            '\r' => dst.push_str("\\r").map_err(|_| ())?,
            '\t' => dst.push_str("\\t").map_err(|_| ())?,
            c if c <= '\u{1f}' => {
                let _ = core::fmt::write(dst, format_args!("\\u{:04x}", c as u32));
            }
            _ => dst.push(ch).map_err(|_| ())?,
        }
    }
    Ok(())
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = core::str::from_utf8(headers).ok()?;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value.trim().parse::<usize>().ok();
        }
    }
    None
}

fn parse_form_field(body: &[u8], key: &str) -> Option<String<MAX_MESSAGE_LEN>> {
    let txt = core::str::from_utf8(body).ok()?;
    for pair in txt.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        if k == key {
            return Some(url_decode(v));
        }
    }
    None
}

fn url_decode(input: &str) -> String<MAX_MESSAGE_LEN> {
    let mut out = String::<MAX_MESSAGE_LEN>::new();
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' {
            let _ = out.push(' ');
            i += 1;
            continue;
        }
        if b == b'%'
            && i + 2 < bytes.len()
            && let (Some(a), Some(c)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            let decoded = (a << 4) | c;
            let _ = out.push(decoded as char);
            i += 3;
            continue;
        }
        let _ = out.push(b as char);
        i += 1;
    }
    out
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rx_entry(text: &str, rssi: i16, snr: i16) -> MessageEntry {
        let mut msg = String::<MAX_MESSAGE_LEN>::new();
        let _ = msg.push_str(text);
        MessageEntry {
            direction: MessageDirection::Rx,
            text: msg,
            rssi: Some(rssi),
            snr: Some(snr),
        }
    }

    #[test]
    fn parse_request_meta_extracts_last_event_id_and_strips_query() {
        let req = b"GET /events?x=1 HTTP/1.1\r\nHost: tbeam\r\nLast-Event-ID: 42\r\n\r\n";
        let header_end = find_bytes(req, b"\r\n\r\n").unwrap() + 4;
        let meta = parse_request_meta(req, header_end).expect("request should parse");

        assert!(matches!(meta.method, HttpMethod::Get));
        assert_eq!(meta.path, "/events");
        assert_eq!(meta.last_event_id, Some(42));
        assert_eq!(meta.body, b"");
    }

    #[test]
    fn parse_request_meta_without_last_event_id_is_none() {
        let req = b"GET /events HTTP/1.1\r\nHost: tbeam\r\n\r\n";
        let header_end = find_bytes(req, b"\r\n\r\n").unwrap() + 4;
        let meta = parse_request_meta(req, header_end).expect("request should parse");

        assert_eq!(meta.last_event_id, None);
    }

    #[test]
    fn sse_snapshot_payload_contains_type_and_entries() {
        let mut log = Vec::<MessageEntry, MAX_LOG_ENTRIES>::new();
        let _ = log.push(rx_entry("hello", -70, 8));

        let payload = render_sse_snapshot_payload(&log);
        assert!(payload.contains("\"type\":\"snapshot\""));
        assert!(payload.contains("\"entries\":["));
        assert!(payload.contains("\"direction\":\"RX\""));
        assert!(payload.contains("\"text\":\"hello\""));
    }

    #[test]
    fn sse_append_payload_contains_one_entry_and_escapes() {
        let entry = rx_entry("\"x\"\\\n", -55, 6);
        let payload = render_sse_append_payload(&entry);

        assert!(payload.contains("\"type\":\"append\""));
        assert!(payload.contains("\"entry\":"));
        assert!(payload.contains("\\\"x\\\"\\\\\\n"));
    }

    #[test]
    fn parse_form_field_skips_malformed_and_decodes_plus() {
        let body = b"bad&other=1&msg=hello+world";
        let parsed = parse_form_field(body, "msg").expect("msg should parse");
        assert_eq!(parsed.as_str(), "hello world");
    }
}

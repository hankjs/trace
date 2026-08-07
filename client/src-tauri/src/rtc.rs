//! WebRTC P2P 终端数据面（协议对齐 handy/docs/p2p-terminal.md）。
//!
//! - 一次 `rtc_accept_offer` = 一个 PeerConnection，后台 tokio 任务驱动
//! - str0m sans-io：poll_output → Transmit/Timeout/Event；socket 收包喂 Input::Receive
//! - non-trickle SDP：host candidate 同步添加，answer 即带全量 candidate
//! - 数据面单行 JSON，字节 base64
//!
//! 注意：`Input::Receive.destination` 必须报 candidate 地址，不是 socket.local_addr()。

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use str0m::change::SdpOffer;
use str0m::channel::{ChannelData, ChannelId};
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tauri::{AppHandle, Emitter, State};
use tokio::net::UdpSocket;

use crate::terminal::TermManager;

const CHANNEL_LABEL: &str = "term";
const PUMP_TICK: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(text: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|e| format!("不是合法 base64: {e}"))
}

/// 通知前端：某终端被 app 远程占用 / 释放 / 尺寸变化（遮罩 + 抑制本地 fit）
#[derive(Clone, Serialize)]
struct TermRemoteEvent {
    id: String,
    active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    cols: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u16>,
}

fn emit_term_remote(
    app: &AppHandle,
    id: &str,
    active: bool,
    cols: Option<u16>,
    rows: Option<u16>,
) {
    let _ = app.emit(
        "term-remote",
        TermRemoteEvent {
            id: id.to_string(),
            active,
            cols,
            rows,
        },
    );
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    Open {
        term_id: Option<String>,
        cols: Option<u16>,
        rows: Option<u16>,
        cwd: Option<String>,
    },
    Input { data_b64: String },
    Resize { cols: u16, rows: u16 },
    Close,
    Ping,
    Resync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    Snapshot { seq: u64, ansi_b64: String },
    Output { seq: u64, data_b64: String },
    Closed { reason: String },
    Pong,
}

fn decode_client_msg(data: &[u8]) -> Result<ClientMsg, String> {
    serde_json::from_slice(data).map_err(|e| format!("协议消息不是合法 JSON: {e}"))
}

fn encode_server_msg(msg: &ServerMsg) -> Result<String, String> {
    serde_json::to_string(msg).map_err(|e| format!("协议消息序列化失败: {e}"))
}

/// 收 offer SDP → 建 PeerConnection → 立即回 answer SDP；数据面后台跑。
pub async fn accept_offer(
    app: AppHandle,
    term: Arc<TermManager>,
    offer_sdp: &str,
) -> Result<String, String> {
    let offer =
        SdpOffer::from_sdp_string(offer_sdp).map_err(|e| format!("offer SDP 解析失败: {e}"))?;

    let ip = select_host_address();
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|e| format!("UDP socket 绑定失败: {e}"))?;
    let port = socket
        .local_addr()
        .map_err(|e| format!("UDP socket 地址获取失败: {e}"))?
        .port();
    let cand_addr = SocketAddr::new(IpAddr::V4(ip), port);

    let mut rtc = Rtc::builder().build(Instant::now());
    let candidate =
        Candidate::host(cand_addr, "udp").map_err(|e| format!("host candidate 无效: {e}"))?;
    rtc.add_local_candidate(candidate);

    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("offer 不合法: {e}"))?;
    let answer_sdp = answer.to_sdp_string();

    tokio::spawn(drive(app, rtc, socket, cand_addr, term));
    Ok(answer_sdp)
}

/// Tauri command：长轮询收到 rtc_signal 时调用（offerSdp → answer SDP）
#[tauri::command]
pub async fn rtc_accept_offer(
    app: AppHandle,
    term: State<'_, Arc<TermManager>>,
    offer_sdp: String,
) -> Result<String, String> {
    accept_offer(app, term.inner().clone(), &offer_sdp).await
}

fn select_host_address() -> Ipv4Addr {
    if let Ok(s) = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = s.local_addr() {
                if let IpAddr::V4(ip) = addr.ip() {
                    if !ip.is_loopback() {
                        return ip;
                    }
                }
            }
        }
    }
    Ipv4Addr::LOCALHOST
}

struct TermBridge {
    term_id: Option<String>,
    cid: Option<ChannelId>,
    last_seq: u64,
    opened: bool,
    connected: bool,
    needs_snapshot: bool,
    done: bool,
}

impl TermBridge {
    fn new() -> Self {
        Self {
            term_id: None,
            cid: None,
            last_seq: 0,
            opened: false,
            connected: false,
            needs_snapshot: false,
            done: false,
        }
    }
}

fn send_msg(rtc: &mut Rtc, bridge: &TermBridge, msg: &ServerMsg) -> bool {
    let Some(cid) = bridge.cid else {
        return false;
    };
    let Ok(json) = encode_server_msg(msg) else {
        return false;
    };
    let Some(mut channel) = rtc.channel(cid) else {
        return false;
    };
    match channel.write(false, json.as_bytes()) {
        Ok(accepted) => accepted,
        Err(e) => {
            tracing::warn!("DataChannel 写入失败: {e}");
            false
        }
    }
}

fn send_closed(rtc: &mut Rtc, bridge: &mut TermBridge, reason: impl Into<String>) {
    send_msg(
        rtc,
        bridge,
        &ServerMsg::Closed {
            reason: reason.into(),
        },
    );
    bridge.done = true;
}

fn send_snapshot(rtc: &mut Rtc, term: &TermManager, bridge: &mut TermBridge) {
    let Some(id) = bridge.term_id.clone() else {
        return;
    };
    match term.term_ansi_screen(&id) {
        Ok((ansi, seq)) => {
            let msg = ServerMsg::Snapshot {
                seq,
                ansi_b64: b64_encode(&ansi),
            };
            if send_msg(rtc, bridge, &msg) {
                bridge.last_seq = seq;
                bridge.needs_snapshot = false;
            } else {
                bridge.needs_snapshot = true;
            }
        }
        Err(e) => send_closed(rtc, bridge, e),
    }
}

fn handle_open(
    app: &AppHandle,
    rtc: &mut Rtc,
    term: &TermManager,
    bridge: &mut TermBridge,
    term_id: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    _cwd: Option<String>,
) {
    // 只附着桌面已开终端（admin 从列表选）；不在 RTC 侧新建 PTY
    let Some(id) = term_id else {
        send_closed(rtc, bridge, "请先在桌面 client 打开终端并选择会话");
        return;
    };
    match term.term_alive(&id) {
        Some(true) => {
            // 尺寸以 app 为准，覆盖本地 client 容器尺寸
            if let (Some(c), Some(r)) = (cols, rows) {
                if term.term_size(&id).ok() != Some((c, r)) {
                    if let Err(e) = term.term_resize_inner(&id, c, r) {
                        tracing::warn!(term_id = %id, "RTC attach resize 失败: {e}");
                    }
                }
            }
            tracing::info!(term_id = %id, "RTC 终端会话已附着");
            emit_term_remote(app, &id, true, cols, rows);
            bridge.term_id = Some(id);
            bridge.opened = true;
            send_snapshot(rtc, term, bridge);
        }
        Some(false) => send_closed(rtc, bridge, "终端进程已退出"),
        None => send_closed(rtc, bridge, "terminal not found"),
    }
}

fn handle_client_msg(
    app: &AppHandle,
    rtc: &mut Rtc,
    term: &TermManager,
    bridge: &mut TermBridge,
    data: &[u8],
) {
    let msg = match decode_client_msg(data) {
        Ok(msg) => msg,
        Err(e) => {
            tracing::warn!("RTC 协议消息无法解析: {e}");
            return;
        }
    };
    match msg {
        ClientMsg::Open {
            term_id,
            cols,
            rows,
            cwd,
        } => handle_open(app, rtc, term, bridge, term_id, cols, rows, cwd),
        ClientMsg::Ping => {
            send_msg(rtc, bridge, &ServerMsg::Pong);
        }
        ClientMsg::Resync => send_snapshot(rtc, term, bridge),
        ClientMsg::Input { data_b64 } => {
            let Some(id) = bridge.term_id.clone() else {
                return;
            };
            match b64_decode(&data_b64) {
                Ok(bytes) => {
                    if let Err(e) = term.term_write_bytes(&id, &bytes) {
                        send_closed(rtc, bridge, e);
                    }
                }
                Err(e) => tracing::warn!("input 消息 {e}"),
            }
        }
        ClientMsg::Resize { cols, rows } => {
            let Some(id) = bridge.term_id.clone() else {
                return;
            };
            if let Err(e) = term.term_resize_inner(&id, cols, rows) {
                tracing::warn!(term_id = %id, "RTC resize 失败: {e}");
            } else {
                // 同步前端遮罩下的 xterm 显示尺寸，并刷新 sticky 占用
                emit_term_remote(app, &id, true, Some(cols), Some(rows));
            }
        }
        ClientMsg::Close => {
            if let Some(id) = bridge.term_id.take() {
                emit_term_remote(app, &id, false, None, None);
            }
            send_closed(rtc, bridge, "closed by peer");
        }
    }
}

fn pump(app: &AppHandle, rtc: &mut Rtc, term: &TermManager, bridge: &mut TermBridge) {
    let Some(id) = bridge.term_id.clone() else {
        return;
    };

    match term.term_alive(&id) {
        Some(true) => {}
        Some(false) => {
            emit_term_remote(app, &id, false, None, None);
            bridge.term_id = None;
            send_closed(rtc, bridge, "终端进程已退出");
            return;
        }
        None => {
            emit_term_remote(app, &id, false, None, None);
            bridge.term_id = None;
            send_closed(rtc, bridge, "terminal not found");
            return;
        }
    }

    if bridge.needs_snapshot {
        send_snapshot(rtc, term, bridge);
        return;
    }

    match term.term_deltas(&id, bridge.last_seq) {
        Ok((deltas, contiguous)) => {
            if !contiguous {
                bridge.needs_snapshot = true;
                return;
            }
            for delta in deltas {
                let msg = ServerMsg::Output {
                    seq: delta.seq,
                    data_b64: b64_encode(&delta.data),
                };
                if send_msg(rtc, bridge, &msg) {
                    bridge.last_seq = delta.seq;
                } else {
                    bridge.needs_snapshot = true;
                    return;
                }
            }
        }
        Err(e) => {
            emit_term_remote(app, &id, false, None, None);
            bridge.term_id = None;
            send_closed(rtc, bridge, e);
        }
    }
}

async fn drive(
    app: AppHandle,
    mut rtc: Rtc,
    socket: UdpSocket,
    cand_addr: SocketAddr,
    term: Arc<TermManager>,
) {
    let mut bridge = TermBridge::new();
    let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut next_pump = Instant::now();
    let mut buf = vec![0u8; 2048];

    loop {
        if !rtc.is_alive() {
            break;
        }

        let now = Instant::now();
        if bridge.opened && !bridge.done && now >= next_pump {
            pump(&app, &mut rtc, &term, &mut bridge);
            next_pump = now + PUMP_TICK;
        }
        if !bridge.connected && now >= connect_deadline {
            tracing::warn!("RTC 建连超时（{CONNECT_TIMEOUT:?}），放弃");
            break;
        }

        let timeout = loop {
            match rtc.poll_output() {
                Ok(Output::Transmit(t)) => {
                    if let Err(e) = socket.send_to(&t.contents, t.destination).await {
                        tracing::warn!("RTC UDP 发送失败: {e}");
                    }
                }
                Ok(Output::Timeout(t)) => break t,
                Ok(Output::Event(e)) => match e {
                    Event::Connected => {
                        tracing::info!("RTC 通道已建立（ICE+DTLS）");
                        bridge.connected = true;
                    }
                    Event::IceConnectionStateChange(s) => {
                        tracing::info!("RTC ICE 状态: {s:?}");
                        if s == IceConnectionState::Disconnected {
                            rtc.disconnect();
                        }
                    }
                    Event::ChannelOpen(cid, label) => {
                        if label == CHANNEL_LABEL && bridge.cid.is_none() {
                            tracing::info!("RTC DataChannel 已打开: {label}");
                            bridge.cid = Some(cid);
                        } else {
                            tracing::warn!("忽略多余 DataChannel: {label}");
                        }
                    }
                    Event::ChannelData(ChannelData { id, data, .. }) => {
                        if Some(id) == bridge.cid {
                            handle_client_msg(&app, &mut rtc, &term, &mut bridge, &data);
                        }
                    }
                    Event::ChannelClose(id) => {
                        if Some(id) == bridge.cid {
                            tracing::info!("RTC DataChannel 被对端关闭");
                            bridge.done = true;
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    tracing::warn!("RTC poll_output 失败: {e}");
                    // 异常退出前释放占用
                    if let Some(id) = bridge.term_id.take() {
                        emit_term_remote(&app, &id, false, None, None);
                    }
                    return;
                }
            }
        };

        if bridge.done {
            break;
        }

        let now = Instant::now();
        let wake = if bridge.opened {
            timeout.min(next_pump)
        } else {
            timeout
        };
        let dur = wake
            .saturating_duration_since(now)
            .max(Duration::from_millis(1));
        match tokio::time::timeout(dur, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, source))) => {
                if let Ok(contents) = buf[..n].try_into() {
                    let input = Input::Receive(
                        Instant::now(),
                        Receive {
                            proto: Protocol::Udp,
                            source,
                            destination: cand_addr,
                            contents,
                        },
                    );
                    if let Err(e) = rtc.handle_input(input) {
                        tracing::warn!("RTC 收包处理失败: {e}");
                        rtc.disconnect();
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!("RTC UDP 接收失败: {e}");
                break;
            }
            Err(_) => {}
        }
        if let Err(e) = rtc.handle_input(Input::Timeout(Instant::now())) {
            tracing::warn!("RTC 计时推进失败: {e}");
            break;
        }
    }

    // 连接结束：释放前端遮罩
    if let Some(id) = bridge.term_id.take() {
        emit_term_remote(&app, &id, false, None, None);
    }
    tracing::info!(term_id = ?bridge.term_id, "RTC 驱动任务退出");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_decode_roundtrip() {
        let msg = decode_client_msg(br#"{"type":"open","cols":120,"rows":30}"#).unwrap();
        assert_eq!(
            msg,
            ClientMsg::Open {
                term_id: None,
                cols: Some(120),
                rows: Some(30),
                cwd: None
            }
        );
        assert_eq!(
            decode_client_msg(br#"{"type":"input","data_b64":"aGk="}"#).unwrap(),
            ClientMsg::Input {
                data_b64: "aGk=".into()
            }
        );
        assert_eq!(decode_client_msg(br#"{"type":"ping"}"#).unwrap(), ClientMsg::Ping);
        assert!(decode_client_msg(b"not json").is_err());
    }

    #[test]
    fn server_msg_encode_roundtrip() {
        let json = encode_server_msg(&ServerMsg::Pong).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
        let bytes = b"echo \x1b[31mhi\x03";
        assert_eq!(b64_decode(&b64_encode(bytes)).unwrap(), bytes);
    }

    #[test]
    fn invalid_offer_sdp_is_rejected() {
        // accept_offer 依赖 AppHandle；SDP 解析失败路径与之一致，直接测 from_sdp_string
        assert!(SdpOffer::from_sdp_string("not an sdp").is_err());
    }
}

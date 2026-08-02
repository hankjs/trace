//! 飞书 WS 长连接客户端（pbbp2 协议，protobuf 帧）。
//!
//! 协议要点（参考飞书官方文档与社区实现逆向）：
//! 1. POST /callback/ws/endpoint 用 app_id/app_secret 换 wss 地址与 ClientConfig
//! 2. 连接后每 PingInterval 秒发一个 method=0 的 ping 控制帧（protobuf 二进制）
//! 3. 服务端推 method=1 数据帧：headers.type = "event" | "card"，payload 为 JSON
//! 4. 每个数据帧必须回一个响应帧：同帧头，payload = {"code":0,"headers":{...},"data":...}
//!    card 帧的 data 可携带 toast/卡片更新等回调响应
//!
//! Frame 的 proto 定义（proto2，字段号即线格式）：
//!   message Header { required string key = 1; required string value = 2; }
//!   message Frame  { required uint64 SeqID = 1; required uint64 LogID = 2;
//!                    required int32 service = 3; required int32 method = 4;
//!                    repeated Header headers = 5; optional string payload_encoding = 6;
//!                    optional string payload_type = 7; optional bytes payload = 8;
//!                    optional string LogIDNew = 9; }

use crate::feishu::{callback, router};
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use hank_db::FeishuAccount;
use prost::Message;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const ENDPOINT_URL: &str = "/callback/ws/endpoint";
/// 心跳超时：超过该时长没收到任何帧视为死连接，触发重连
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(120);
/// ClientConfig 缺失时的默认 ping 间隔
const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(120);

// ── pbbp2 帧定义（prost derive 直接生成线格式，无需 protoc）──

#[derive(Clone, PartialEq, Message)]
pub struct Header {
    #[prost(string, required, tag = "1")]
    pub key: String,
    #[prost(string, required, tag = "2")]
    pub value: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct Frame {
    #[prost(uint64, required, tag = "1")]
    pub seq_id: u64,
    #[prost(uint64, required, tag = "2")]
    pub log_id: u64,
    #[prost(int32, required, tag = "3")]
    pub service: i32,
    #[prost(int32, required, tag = "4")]
    pub method: i32,
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<Header>,
    #[prost(string, optional, tag = "6")]
    pub payload_encoding: Option<String>,
    #[prost(string, optional, tag = "7")]
    pub payload_type: Option<String>,
    #[prost(bytes = "vec", optional, tag = "8")]
    pub payload: Option<Vec<u8>>,
    #[prost(string, optional, tag = "9")]
    pub log_id_new: Option<String>,
}

impl Frame {
    fn header_value(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.key == key)
            .map(|h| h.value.as_str())
    }

    fn ping(service_id: i32) -> Self {
        Self {
            seq_id: 0,
            log_id: 0,
            service: service_id,
            method: 0,
            headers: vec![Header {
                key: "type".to_string(),
                value: "ping".to_string(),
            }],
            payload_encoding: None,
            payload_type: None,
            payload: None,
            log_id_new: None,
        }
    }

    /// 基于收到的数据帧构建响应帧（ACK/卡片回调响应）
    fn into_response(mut self, data: Value, biz_rt_ms: u128) -> Self {
        self.headers.push(Header {
            key: "biz_rt".to_string(),
            value: biz_rt_ms.to_string(),
        });
        self.payload = Some(
            json!({
                "code": 0,
                "headers": {},
                "data": data,
            })
            .to_string()
            .into_bytes(),
        );
        self
    }
}

// ── 端点协商 ──

#[derive(Debug, Deserialize)]
struct EndpointResp {
    code: i64,
    msg: String,
    data: Option<EndpointData>,
}

#[derive(Debug, Deserialize)]
struct EndpointData {
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "ClientConfig")]
    client_config: Option<ClientConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClientConfig {
    #[serde(rename = "PingInterval")]
    pub ping_interval: Option<u64>,
}

async fn get_conn_url(account: &FeishuAccount) -> Result<(String, ClientConfig)> {
    let resp = reqwest::Client::new()
        .post(format!("{}{}", super::api::FEISHU_BASE_URL, ENDPOINT_URL))
        .header("locale", "zh")
        .json(&json!({ "AppID": account.app_id, "AppSecret": account.app_secret }))
        .send()
        .await?
        .json::<EndpointResp>()
        .await?;
    if resp.code != 0 {
        bail!("飞书获取 WS 端点失败 code={} msg={}", resp.code, resp.msg);
    }
    let data = resp.data.ok_or_else(|| anyhow!("WS 端点响应缺少 data"))?;
    let url = data
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow!("WS 端点响应缺少 URL"))?;
    Ok((
        url,
        data.client_config.unwrap_or(ClientConfig {
            ping_interval: None,
        }),
    ))
}

fn parse_service_id(conn_url: &str) -> Result<i32> {
    let url = reqwest::Url::parse(conn_url).context("解析 wss 地址失败")?;
    url.query_pairs()
        .find(|(k, _)| k == "service_id")
        .and_then(|(_, v)| v.parse::<i32>().ok())
        .ok_or_else(|| anyhow!("wss 地址缺少 service_id"))
}

// ── 连接主循环 ──

/// 建立一次长连接并运行到断开/cancel。返回 Err 表示需要重连。
pub async fn connect_and_run(
    state: Arc<AppState>,
    account: FeishuAccount,
    token: CancellationToken,
) -> Result<()> {
    let (conn_url, client_config) = get_conn_url(&account).await?;
    let service_id = parse_service_id(&conn_url)?;
    let ping_interval = client_config
        .ping_interval
        .filter(|&v| v > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_PING_INTERVAL);

    let (ws, _resp) = tokio_tungstenite::connect_async(&conn_url).await?;
    tracing::info!("feishu ws connected, service_id={service_id}, ping={ping_interval:?}");
    let (mut sink, mut stream) = ws.split();

    // 出方向帧队列：数据帧的响应从 dispatch 任务回到这里统一发送
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Frame>();
    let mut ping_tick = tokio::time::interval(ping_interval);
    let mut last_activity = Instant::now();
    let mut heartbeat_tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = token.cancelled() => {
                tracing::info!("feishu ws cancelled, closing");
                let _ = sink.close().await;
                return Ok(());
            }
            msg = stream.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(data))) => {
                        last_activity = Instant::now();
                        let frame = Frame::decode(data.as_ref())
                            .context("protobuf Frame 解码失败")?;
                        match frame.method {
                            0 => {
                                // 控制帧（pong）：心跳回包，无需处理
                                tracing::trace!("feishu ws pong");
                            }
                            1 => {
                                let started = Instant::now();
                                let out = out_tx.clone();
                                let state = state.clone();
                                let account = account.clone();
                                tokio::spawn(async move {
                                    let resp = dispatch_data_frame(state, account, frame, started.elapsed()).await;
                                    let _ = out.send(resp);
                                });
                            }
                            m => tracing::debug!(method = m, "feishu ws: unknown frame method"),
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Ping(p))) => {
                        last_activity = Instant::now();
                        sink.send(tokio_tungstenite::tungstenite::Message::Pong(p)).await?;
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(reason))) => {
                        bail!("feishu ws closed by peer: {reason:?}");
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => bail!("feishu ws error: {e}"),
                    None => bail!("feishu ws stream ended"),
                }
            }
            Some(frame) = out_rx.recv() => {
                sink.send(tokio_tungstenite::tungstenite::Message::Binary(
                    frame.encode_to_vec(),
                ))
                .await?;
            }
            _ = ping_tick.tick() => {
                let ping = Frame::ping(service_id);
                sink.send(tokio_tungstenite::tungstenite::Message::Binary(
                    ping.encode_to_vec(),
                ))
                .await?;
            }
            _ = heartbeat_tick.tick() => {
                if last_activity.elapsed() > HEARTBEAT_TIMEOUT {
                    bail!("feishu ws heartbeat timeout ({HEARTBEAT_TIMEOUT:?})");
                }
            }
        }
    }
}

/// 分发数据帧：event 立即 ACK 后台处理；card 等待回调结果构造响应。
async fn dispatch_data_frame(
    state: Arc<AppState>,
    account: FeishuAccount,
    frame: Frame,
    started: Duration,
) -> Frame {
    let frame_type = frame.header_value("type").unwrap_or("").to_string();
    let message_id = frame.header_value("message_id").unwrap_or("").to_string();
    let payload = frame.payload.clone().unwrap_or_default();

    let data = match frame_type.as_str() {
        "event" => {
            // 事件处理可能很慢（要跑 agent），立即 ACK，后台分发
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(e) = router::handle_event(state, account, &payload).await {
                    tracing::warn!("feishu: handle event failed: {e:#}");
                }
            });
            Value::Null
        }
        "card" => match callback::handle_card_action(state, account, &payload).await {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(message_id, "feishu: card action failed: {e:#}");
                json!({ "toast": { "type": "error", "content": "操作处理失败，请直接文字回复" } })
            }
        },
        other => {
            tracing::debug!(frame_type = other, "feishu ws: unknown data frame type");
            Value::Null
        }
    };

    frame.into_response(data, started.as_millis())
}

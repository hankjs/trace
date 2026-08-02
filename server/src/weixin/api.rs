//! ilink HTTP client，协议仿腾讯官方参考实现（/tmp/ocwx/package/dist/src/api/api.js）。

use anyhow::{anyhow, Result};
use hank_db::WeixinAccount;
use md5::Digest;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

/// QR 登录固定入口
pub const QR_API_BASE: &str = "https://ilinkai.weixin.qq.com";
const ILINK_APP_ID: &str = "bot";
/// iLink-App-ClientVersion: 2.4.0 编码为 (2<<16)|(4<<8)|0
const ILINK_APP_CLIENT_VERSION: u32 = (2 << 16) | (4 << 8);
/// getupdates 返回此 errcode 表示 bot_token 过期
pub const ERRCODE_TOKEN_EXPIRED: i64 = -14;

const NORMAL_TIMEOUT: Duration = Duration::from_secs(15);
const LONG_POLL_TIMEOUT: Duration = Duration::from_secs(40);
/// 微信单条文本过长时的切片长度（字符数）
const TEXT_CHUNK_CHARS: usize = 1800;
/// 微信 CDN 基础地址：getuploadurl 未返回 upload_full_url 时回退拼接上传地址
const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
/// CDN 上传超时（媒体文件较大，给足时间）
const CDN_UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// CDN 上传失败重试次数（仅 5xx/网络错误重试，4xx 直接失败）
const CDN_UPLOAD_RETRIES: u32 = 3;

/// proto: UploadMediaType
const MEDIA_TYPE_IMAGE: u32 = 1;
const MEDIA_TYPE_VIDEO: u32 = 2;
const MEDIA_TYPE_FILE: u32 = 3;

/// proto: MessageItemType（媒体）
const ITEM_TYPE_IMAGE: u32 = 2;
const ITEM_TYPE_FILE: u32 = 4;
const ITEM_TYPE_VIDEO: u32 = 5;

#[derive(Clone)]
pub struct IlinkClient {
    http: reqwest::Client,
}

/// X-WECHAT-UIN: base64(随机 uint32 的十进制字符串)。随机数取自 uuid。
fn random_wechat_uin() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let n = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    base64_encode(n.to_string().as_bytes())
}

/// 极简 base64（标准字符集，带 padding），避免新增依赖。
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        out.push(CHARS[(b[0] >> 2) as usize] as char);
        out.push(CHARS[(((b[0] & 0x03) << 4) | (b[1] >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[(((b[1] & 0x0f) << 2) | (b[2] >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(b[2] & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn base_info() -> Value {
    json!({
        "channel_version": "1.0.0",
        "bot_agent": "Trace/1.0",
    })
}

/// 极简 hex（小写），避免新增依赖。
fn hex_encode(input: &[u8]) -> String {
    input.iter().map(|b| format!("{b:02x}")).collect()
}

/// AES-128-ECB 密文长度（PKCS7 补齐到 16 字节边界，整块时额外加一块）
fn aes_ecb_padded_size(plaintext_size: usize) -> usize {
    (plaintext_size + 1).div_ceil(16) * 16
}

/// AES-128-ECB 加密（PKCS7 padding），与参考实现 aes-ecb.js 一致。
fn aes128_ecb_encrypt(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
    ecb::Encryptor::<aes::Aes128>::new(key.into()).encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

/// 按扩展名推断 proto: UploadMediaType
fn media_type_of(file_name: &str) -> u32 {
    let ext = file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "heif" => MEDIA_TYPE_IMAGE,
        "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" => MEDIA_TYPE_VIDEO,
        _ => MEDIA_TYPE_FILE,
    }
}

#[derive(Debug, Deserialize)]
pub struct QrcodeResponse {
    pub qrcode: Option<String>,
    pub qrcode_img_content: Option<String>,
    #[serde(default)]
    pub errmsg: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QrStatusResponse {
    pub status: String,
    pub bot_token: Option<String>,
    pub ilink_bot_id: Option<String>,
    pub baseurl: Option<String>,
    pub ilink_user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextItem {
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageItem {
    #[serde(rename = "type")]
    pub item_type: Option<u32>,
    pub text_item: Option<TextItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IlinkMessage {
    pub from_user_id: Option<String>,
    #[allow(dead_code)]
    pub to_user_id: Option<String>,
    pub message_type: Option<u32>,
    #[serde(default)]
    pub item_list: Vec<MessageItem>,
    pub context_token: Option<String>,
    #[allow(dead_code)]
    pub message_id: Option<u64>,
}

impl IlinkMessage {
    /// 提取文本内容（item type=1 为文本）
    pub fn text(&self) -> Option<String> {
        let text: String = self
            .item_list
            .iter()
            .filter(|i| i.item_type == Some(1))
            .filter_map(|i| i.text_item.as_ref().and_then(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("");
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct GetUpdatesResponse {
    pub ret: Option<i64>,
    pub errcode: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub errmsg: Option<String>,
    #[serde(default)]
    pub msgs: Vec<IlinkMessage>,
    pub get_updates_buf: Option<String>,
    #[allow(dead_code)]
    pub longpolling_timeout_ms: Option<u64>,
}

impl GetUpdatesResponse {
    pub fn token_expired(&self) -> bool {
        self.errcode == Some(ERRCODE_TOKEN_EXPIRED) || self.ret == Some(ERRCODE_TOKEN_EXPIRED)
    }
}

impl IlinkClient {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    fn common_headers(&self) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("iLink-App-Id", ILINK_APP_ID.parse().unwrap());
        h.insert(
            "iLink-App-ClientVersion",
            ILINK_APP_CLIENT_VERSION.to_string().parse().unwrap(),
        );
        h
    }

    fn post_headers(&self, token: Option<&str>) -> reqwest::header::HeaderMap {
        let mut h = self.common_headers();
        h.insert("Content-Type", "application/json".parse().unwrap());
        h.insert("AuthorizationType", "ilink_bot_token".parse().unwrap());
        h.insert("X-WECHAT-UIN", random_wechat_uin().parse().unwrap());
        if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
            h.insert(
                "Authorization",
                format!("Bearer {}", t.trim()).parse().unwrap(),
            );
        }
        h
    }

    async fn post(
        &self,
        base: &str,
        endpoint: &str,
        body: &Value,
        token: Option<&str>,
        timeout: Duration,
    ) -> Result<String> {
        let url = format!("{}/{}", base.trim_end_matches('/'), endpoint);
        let res = self
            .http
            .post(&url)
            .headers(self.post_headers(token))
            .body(body.to_string())
            .timeout(timeout)
            .send()
            .await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            return Err(anyhow!("ilink POST {endpoint} {status}: {text}"));
        }
        Ok(text)
    }

    /// 获取登录二维码。返回 (qrcode, qrcode_img_content)；后者是要展示/扫码的链接。
    pub async fn fetch_qrcode(&self) -> Result<(String, String)> {
        let body = json!({ "local_token_list": [] });
        let text = self
            .post(
                QR_API_BASE,
                "ilink/bot/get_bot_qrcode?bot_type=3",
                &body,
                None,
                NORMAL_TIMEOUT,
            )
            .await?;
        let resp: QrcodeResponse = serde_json::from_str(&text)?;
        match (resp.qrcode, resp.qrcode_img_content) {
            (Some(q), Some(img)) => Ok((q, img)),
            _ => Err(anyhow!(
                "get_bot_qrcode 响应缺少字段: {}",
                resp.errmsg
                    .unwrap_or_else(|| text.chars().take(200).collect())
            )),
        }
    }

    /// 长轮询二维码状态。客户端超时/网络错误视为 wait（与参考实现一致）。
    pub async fn poll_qr_status(&self, base: &str, qrcode: &str) -> Result<QrStatusResponse> {
        let url = format!(
            "{}/ilink/bot/get_qrcode_status?qrcode={}",
            base.trim_end_matches('/'),
            urlencoding_encode(qrcode),
        );
        let result = self
            .http
            .get(&url)
            .headers(self.common_headers())
            .timeout(LONG_POLL_TIMEOUT)
            .send()
            .await;
        match result {
            Ok(res) => {
                let text = res.text().await?;
                Ok(serde_json::from_str(&text)?)
            }
            Err(e) => {
                if e.is_timeout() {
                    Ok(QrStatusResponse {
                        status: "wait".to_string(),
                        bot_token: None,
                        ilink_bot_id: None,
                        baseurl: None,
                        ilink_user_id: None,
                    })
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// 长轮询收取消息。客户端超时返回空响应（ret=0），由调用方直接重试。
    pub async fn get_updates(
        &self,
        account: &WeixinAccount,
        buf: Option<&str>,
    ) -> Result<GetUpdatesResponse> {
        let body = json!({
            "get_updates_buf": buf.unwrap_or(""),
            "base_info": base_info(),
        });
        let result = self
            .post(
                &account.base_url,
                "ilink/bot/getupdates",
                &body,
                Some(&account.bot_token),
                LONG_POLL_TIMEOUT,
            )
            .await;
        match result {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(e) => {
                if e.downcast_ref::<reqwest::Error>()
                    .is_some_and(|r| r.is_timeout())
                {
                    Ok(GetUpdatesResponse {
                        ret: Some(0),
                        get_updates_buf: buf.map(|s| s.to_string()),
                        ..Default::default()
                    })
                } else {
                    Err(e)
                }
            }
        }
    }

    /// 发送文本消息，超长按字符数切片分多条发送。
    pub async fn send_text(
        &self,
        account: &WeixinAccount,
        to_user_id: &str,
        context_token: &str,
        text: &str,
    ) -> Result<()> {
        // 微信会把 \n / \r\n 渲染成空格（实测），U+2028 才能显示为换行
        let text = text.replace("\r\n", "\u{2028}").replace('\n', "\u{2028}");
        let mut chunk = String::new();
        let mut chunks: Vec<String> = Vec::new();
        for c in text.chars() {
            chunk.push(c);
            if chunk.chars().count() >= TEXT_CHUNK_CHARS {
                chunks.push(std::mem::take(&mut chunk));
            }
        }
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        if chunks.is_empty() {
            return Ok(());
        }
        for part in chunks {
            // client_id 每条必须唯一：ilink 用它去重，缺失时后续消息会被静默丢弃。
            // message_type=2 (BOT)、message_state=2 (FINISH) 与官方参考实现一致。
            let client_id = format!("trace-weixin-{}", uuid::Uuid::new_v4());
            let body = json!({
                "msg": {
                    "from_user_id": "",
                    "to_user_id": to_user_id,
                    "client_id": client_id,
                    "message_type": 2,
                    "message_state": 2,
                    "context_token": context_token,
                    "item_list": [{ "type": 1, "text_item": { "text": part } }],
                },
                "base_info": base_info(),
            });
            let text_resp = self
                .post(
                    &account.base_url,
                    "ilink/bot/sendmessage",
                    &body,
                    Some(&account.bot_token),
                    NORMAL_TIMEOUT,
                )
                .await?;
            let resp: Value = serde_json::from_str(&text_resp)?;
            let ret = resp["ret"].as_i64().unwrap_or(0);
            if ret != 0 {
                let errmsg = resp["errmsg"].as_str().unwrap_or("(none)");
                return Err(anyhow!("sendmessage ret={ret} errmsg={errmsg}"));
            }
            tracing::info!(
                to = to_user_id,
                client_id,
                len = part.chars().count(),
                "weixin: message sent"
            );
        }
        Ok(())
    }

    /// 发送媒体文件：按扩展名路由到图片/视频/文件附件。
    /// 流程与官方参考实现一致：getuploadurl 换取上传地址 → AES-128-ECB 加密后 POST 到 CDN
    /// → sendmessage 发送引用 CDN 的媒体 item。
    pub async fn send_media(
        &self,
        account: &WeixinAccount,
        to_user_id: &str,
        context_token: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<()> {
        let rawsize = data.len();
        let rawfilemd5 = format!("{:x}", md5::Md5::digest(data));
        let filesize = aes_ecb_padded_size(rawsize);
        let filekey = hex_encode(uuid::Uuid::new_v4().as_bytes());
        let aeskey = *uuid::Uuid::new_v4().as_bytes();
        let aeskey_hex = hex_encode(&aeskey);

        let body = json!({
            "filekey": filekey,
            "media_type": media_type_of(file_name),
            "to_user_id": to_user_id,
            "rawsize": rawsize,
            "rawfilemd5": rawfilemd5,
            "filesize": filesize,
            "no_need_thumb": true,
            "aeskey": aeskey_hex,
            "base_info": base_info(),
        });
        let text = self
            .post(
                &account.base_url,
                "ilink/bot/getuploadurl",
                &body,
                Some(&account.bot_token),
                NORMAL_TIMEOUT,
            )
            .await?;
        let resp: Value = serde_json::from_str(&text)?;
        let ret = resp["ret"].as_i64().unwrap_or(0);
        if ret != 0 {
            let errmsg = resp["errmsg"].as_str().unwrap_or("(none)");
            return Err(anyhow!("getuploadurl ret={ret} errmsg={errmsg}"));
        }
        let upload_full_url = resp["upload_full_url"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let upload_param = resp["upload_param"].as_str().filter(|s| !s.is_empty());
        let cdn_url = match (upload_full_url, upload_param) {
            (Some(u), _) => u.to_string(),
            (None, Some(p)) => format!(
                "{CDN_BASE_URL}/upload?encrypted_query_param={}&filekey={}",
                urlencoding_encode(p),
                urlencoding_encode(&filekey)
            ),
            (None, None) => {
                return Err(anyhow!(
                    "getuploadurl 未返回上传地址: {}",
                    text.chars().take(200).collect::<String>()
                ))
            }
        };

        let ciphertext = aes128_ecb_encrypt(&aeskey, data);
        let download_param = self.upload_to_cdn(&cdn_url, &ciphertext).await?;

        // 与参考实现一致：aes_key 为 hex 字符串（ASCII）再 base64
        let media = json!({
            "encrypt_query_param": download_param,
            "aes_key": base64_encode(aeskey_hex.as_bytes()),
            "encrypt_type": 1,
        });
        let item = match media_type_of(file_name) {
            MEDIA_TYPE_IMAGE => json!({
                "type": ITEM_TYPE_IMAGE,
                "image_item": { "media": media, "mid_size": filesize },
            }),
            MEDIA_TYPE_VIDEO => json!({
                "type": ITEM_TYPE_VIDEO,
                "video_item": { "media": media, "video_size": filesize },
            }),
            _ => json!({
                "type": ITEM_TYPE_FILE,
                "file_item": { "media": media, "file_name": file_name, "len": rawsize.to_string() },
            }),
        };
        let client_id = format!("trace-weixin-{}", uuid::Uuid::new_v4());
        let body = json!({
            "msg": {
                "from_user_id": "",
                "to_user_id": to_user_id,
                "client_id": client_id,
                "message_type": 2,
                "message_state": 2,
                "context_token": context_token,
                "item_list": [item],
            },
            "base_info": base_info(),
        });
        let text_resp = self
            .post(
                &account.base_url,
                "ilink/bot/sendmessage",
                &body,
                Some(&account.bot_token),
                NORMAL_TIMEOUT,
            )
            .await?;
        let resp: Value = serde_json::from_str(&text_resp)?;
        let ret = resp["ret"].as_i64().unwrap_or(0);
        if ret != 0 {
            let errmsg = resp["errmsg"].as_str().unwrap_or("(none)");
            return Err(anyhow!("sendmessage ret={ret} errmsg={errmsg}"));
        }
        tracing::info!(
            to = to_user_id,
            client_id,
            file_name,
            rawsize,
            "weixin: media sent"
        );
        Ok(())
    }

    /// POST 密文到 CDN，返回 x-encrypted-param 下载凭证。5xx/网络错误重试，4xx 直接失败。
    async fn upload_to_cdn(&self, url: &str, ciphertext: &[u8]) -> Result<String> {
        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=CDN_UPLOAD_RETRIES {
            let result = self
                .http
                .post(url)
                .header("Content-Type", "application/octet-stream")
                .body(ciphertext.to_vec())
                .timeout(CDN_UPLOAD_TIMEOUT)
                .send()
                .await;
            match result {
                Ok(res) => {
                    let status = res.status();
                    if status.is_success() {
                        return res
                            .headers()
                            .get("x-encrypted-param")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string())
                            .ok_or_else(|| anyhow!("CDN 上传响应缺少 x-encrypted-param 头"));
                    }
                    let err_msg = res
                        .headers()
                        .get("x-error-message")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());
                    let err = anyhow!("CDN 上传失败 {status}: {}", err_msg.unwrap_or_default());
                    if status.is_client_error() {
                        return Err(err);
                    }
                    last_err = Some(err);
                }
                Err(e) => last_err = Some(e.into()),
            }
            if attempt < CDN_UPLOAD_RETRIES {
                tracing::warn!(attempt, "weixin: CDN upload failed, retrying");
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow!("CDN 上传失败")))
    }
}

/// URL query 参数转义（仅需覆盖 qrcode 中的保留字符）。
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Default for IlinkClient {
    fn default() -> Self {
        Self::new()
    }
}

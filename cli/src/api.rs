//! server HTTP API 封装：login / registration / poll / tool-result / notify。
//! 响应统一信封 `{code, msg, data}`，code == 0 为成功；
//! 401 表示 token 失效，由 relogin 重新登录后重放。

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::RwLock;

/// 长轮询超时：server 挂起最长 25s，客户端多留 10s 余量
const POLL_TIMEOUT: Duration = Duration::from_secs(35);

/// 一条待执行的工具调用（GET /api/client/poll 下发）
#[derive(Debug, Clone, Deserialize)]
pub struct ToolCallRequest {
    pub request_id: String,
    pub tool: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

/// 长轮询结果三态，对应 TS 版 PollResult，供退避策略使用
pub enum PollOutcome {
    Ok(Vec<ToolCallRequest>),
    Unauthorized,
    Error(String),
}

/// 统一响应信封
#[derive(Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

pub struct ApiClient {
    http: reqwest::Client,
    server: String,
    token: RwLock<String>,
    username: String,
    password: String,
}

impl ApiClient {
    pub fn new(server: String, username: String, password: String) -> Arc<Self> {
        Arc::new(Self {
            http: reqwest::Client::new(),
            server,
            token: RwLock::new(String::new()),
            username,
            password,
        })
    }

    /// 解析信封：code != 0 视为错误，返回 msg
    fn parse_envelope(body: &str) -> Result<serde_json::Value, String> {
        let env: Envelope =
            serde_json::from_str(body).map_err(|e| format!("响应解析失败: {e}"))?;
        if env.code != 0 {
            return Err(env.msg.unwrap_or_else(|| format!("code={}", env.code)));
        }
        Ok(env.data.unwrap_or(serde_json::Value::Null))
    }

    /// POST /api/auth/login，成功后更新内存 token（不落盘）
    pub async fn login(&self) -> Result<(), String> {
        let res = self
            .http
            .post(format!("{}/api/auth/login", self.server))
            .json(&serde_json::json!({
                "username": self.username,
                "password": self.password,
                "scope": "client",
            }))
            .send()
            .await
            .map_err(|e| format!("login 请求失败: {e}"))?;
        let status = res.status();
        let body = res.text().await.map_err(|e| format!("login 读取响应失败: {e}"))?;
        if status.as_u16() == 401 {
            return Err("login 失败：用户名或密码错误".into());
        }
        let data = Self::parse_envelope(&body)
            .map_err(|e| format!("login 失败（{status}）: {e}"))?;
        let token = data
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or("login 响应缺少 token")?
            .to_string();
        *self.token.write().await = token;
        Ok(())
    }

    /// 清 token 重新登录（401 场景）
    pub async fn relogin(&self) -> Result<(), String> {
        *self.token.write().await = String::new();
        self.login().await
    }

    /// 带鉴权的 JSON 请求；401 时自动 relogin 并重放一次
    async fn authed<Req: serde::Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: Option<&[(&str, &str)]>,
        body: Option<&Req>,
    ) -> Result<reqwest::Response, String> {
        for attempt in 0..2 {
            let token = self.token.read().await.clone();
            let mut req = self
                .http
                .request(method.clone(), format!("{}{}", self.server, path))
                .bearer_auth(token);
            if let Some(q) = query {
                req = req.query(q);
            }
            if let Some(b) = body {
                req = req.json(b);
            }
            let res = req.send().await.map_err(|e| format!("{method} {path} 请求失败: {e}"))?;
            if res.status().as_u16() == 401 && attempt == 0 {
                self.relogin().await?;
                continue;
            }
            return Ok(res);
        }
        unreachable!()
    }

    /// PUT /api/client/registration：注册/更新本节点
    pub async fn register(
        &self,
        client_id: &str,
        hostname: Option<&str>,
        work_dir: Option<&str>,
        agent_backends: &[String],
    ) -> Result<(), String> {
        let res = self
            .authed(
                reqwest::Method::PUT,
                "/api/client/registration",
                None::<&[(&str, &str)]>,
                Some(&serde_json::json!({
                    "client_id": client_id,
                    "hostname": hostname,
                    "work_dir": work_dir,
                    "accept_remote": true,
                    "agent_backends": agent_backends,
                })),
            )
            .await?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        Self::parse_envelope(&body).map_err(|e| format!("registration 失败（{status}）: {e}"))?;
        Ok(())
    }

    /// POST /api/client/agent-event：流式上报本机 Agent 的一行 stdout/stderr。
    pub async fn post_agent_event(
        &self,
        request_id: &str,
        event: &serde_json::Value,
    ) -> Result<(), String> {
        let res = self
            .authed(
                reqwest::Method::POST,
                "/api/client/agent-event",
                None::<&[(&str, &str)]>,
                Some(&serde_json::json!({
                    "request_id": request_id,
                    "event": event,
                })),
            )
            .await?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        Self::parse_envelope(&body)
            .map_err(|e| format!("agent-event 失败（{status}）: {e}"))?;
        Ok(())
    }

    /// GET /api/client/poll：长轮询待执行请求（server 挂起最长 25s）
    pub async fn poll(&self, client_id: &str, agent_backends: &[String]) -> PollOutcome {
        let token = self.token.read().await.clone();
        let agent_backends = agent_backends.join(",");
        let res = self
            .http
            .get(format!("{}/api/client/poll", self.server))
            .bearer_auth(token)
            .query(&[
                ("client_id", client_id),
                ("agent_backends", agent_backends.as_str()),
            ])
            .timeout(POLL_TIMEOUT)
            .send()
            .await;
        let res = match res {
            Ok(r) => r,
            Err(e) => return PollOutcome::Error(format!("poll 请求失败: {e}")),
        };
        if res.status().as_u16() == 401 {
            return PollOutcome::Unauthorized;
        }
        let status = res.status();
        let body = match res.text().await {
            Ok(b) => b,
            Err(e) => return PollOutcome::Error(format!("poll 读取响应失败: {e}")),
        };
        match Self::parse_envelope(&body) {
            Ok(data) => {
                let requests = data
                    .get("requests")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                match serde_json::from_value::<Vec<ToolCallRequest>>(requests) {
                    Ok(reqs) => PollOutcome::Ok(reqs),
                    Err(e) => PollOutcome::Error(format!("poll 响应解析失败: {e}")),
                }
            }
            Err(e) => PollOutcome::Error(format!("poll 失败（{status}）: {e}")),
        }
    }

    /// POST /api/client/tool-result：回传工具执行结果
    pub async fn post_result(&self, request_id: &str, content: &str, is_error: bool) -> Result<(), String> {
        let res = self
            .authed(
                reqwest::Method::POST,
                "/api/client/tool-result",
                None::<&[(&str, &str)]>,
                Some(&serde_json::json!({
                    "request_id": request_id,
                    "content": content,
                    "is_error": is_error,
                })),
            )
            .await?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        Self::parse_envelope(&body).map_err(|e| format!("tool-result 失败（{status}）: {e}"))?;
        Ok(())
    }

    /// POST /api/client/notify：上报终端通知（OSC 9/777/133/BEL 捕获）
    pub async fn post_notify(
        &self,
        client_id: &str,
        term_id: Option<&str>,
        kind: &str,
        title: &str,
        body: &str,
    ) -> Result<(), String> {
        let res = self
            .authed(
                reqwest::Method::POST,
                "/api/client/notify",
                None::<&[(&str, &str)]>,
                Some(&serde_json::json!({
                    "client_id": client_id,
                    "term_id": term_id,
                    "kind": kind,
                    "title": title,
                    "body": body,
                })),
            )
            .await?;
        let status = res.status();
        let body = res.text().await.map_err(|e| e.to_string())?;
        Self::parse_envelope(&body).map_err(|e| format!("notify 失败（{status}）: {e}"))?;
        Ok(())
    }

    /// GET /api/client/online：查询本用户的 client 在线状态（调试用）
    #[allow(dead_code)]
    pub async fn list_online<T: DeserializeOwned>(&self) -> Result<T, String> {
        let res = self
            .authed(reqwest::Method::GET, "/api/client/online", None::<&[(&str, &str)]>, None::<&serde_json::Value>)
            .await?;
        let body = res.text().await.map_err(|e| e.to_string())?;
        let data = Self::parse_envelope(&body)?;
        serde_json::from_value(data).map_err(|e| format!("online 响应解析失败: {e}"))
    }
}

//! 定时任务实现。

use crate::feishu::api::FeishuApi;
use crate::scheduler::TZ;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use chrono::TimeZone;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

/// 单用户简报的信号条数上限（防刷屏）
const MAX_BRIEF_ITEMS: usize = 20;

#[derive(Debug, Deserialize)]
struct SignalsResp {
    items: Vec<SignalItem>,
}

#[derive(Debug, Deserialize)]
struct SignalItem {
    code: String,
    name: String,
    strategy_name: String,
    side_name: String,
    price: Option<f64>,
    reason_text: Option<String>,
}

/// 盘后信号简报：拉 quant 今日信号，按飞书绑定用户各推一份单聊简报。
///
/// 安静原则（agent-os 第 01 篇）：无信号的用户不推；全部无信号则本次执
/// 行静默结束（结果里记 quiet）。每个绑定用户用自己的 JWT 拉取，quant 的
/// 策略可见性过滤（公共 + 自己的策略）天然生效。
pub async fn quant_signal_brief(state: Arc<AppState>) -> Result<Value> {
    let quant = state
        .config
        .quant_a2a
        .as_ref()
        .ok_or_else(|| anyhow!("quant_a2a 未配置，跳过信号简报"))?;
    let base = quant.base_url.trim_end_matches('/');
    let today = TZ.from_utc_datetime(&chrono::Utc::now().naive_utc()).date_naive();

    let bindings = state.db.list_feishu_bindings().await?;
    if bindings.is_empty() {
        return Ok(json!({ "date": today.to_string(), "skipped": true, "reason": "无飞书绑定用户" }));
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut pushed: Vec<String> = Vec::new();
    let mut quiet: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for binding in &bindings {
        let r = push_user_brief(&state, &http, base, binding, today).await;
        match r {
            Ok(true) => pushed.push(binding.username.clone()),
            Ok(false) => quiet.push(binding.username.clone()),
            Err(e) => {
                tracing::warn!(user = %binding.username, "scheduler: 信号简报推送失败: {e:#}");
                errors.push(format!("{}: {e:#}", binding.username));
            }
        }
    }

    Ok(json!({
        "date": today.to_string(),
        "pushed": pushed,
        "quiet": quiet,
        "errors": errors,
    }))
}

/// 给单个绑定用户拉信号并推送。Ok(true)=已推，Ok(false)=无信号保持安静。
async fn push_user_brief(
    state: &Arc<AppState>,
    http: &reqwest::Client,
    base: &str,
    binding: &hank_db::FeishuBindingWithUsername,
    today: chrono::NaiveDate,
) -> Result<bool> {
    let jwt = crate::auth::sign_internal_jwt(&state.jwt_secret, &binding.user_id, &binding.username)
        .context("签内部 JWT 失败")?;

    let resp = http
        .get(format!("{base}/api/signals?date={today}&limit=100"))
        .bearer_auth(&jwt)
        .send()
        .await
        .context("调 quant /api/signals 失败")?;
    if !resp.status().is_success() {
        bail!("quant /api/signals 返回 {}", resp.status());
    }
    let signals = resp.json::<SignalsResp>().await?;
    if signals.items.is_empty() {
        return Ok(false);
    }

    let account = state
        .db
        .get_feishu_account(&binding.account_id)
        .await?
        .ok_or_else(|| anyhow!("飞书账号不存在: {}", binding.account_id))?;
    if !account.enabled {
        bail!("飞书账号已停用: {}", account.app_id);
    }
    let api = FeishuApi::new(&account);
    api.send_text("open_id", &binding.open_id, &format_brief(today, &signals.items))
        .await?;
    Ok(true)
}

/// 简报文本：信号按买入/卖出分组，带策略名与理由。
fn format_brief(today: chrono::NaiveDate, items: &[SignalItem]) -> String {
    let mut lines = vec![format!("📈 盘后信号简报（{today}）"), String::new()];
    for (i, s) in items.iter().take(MAX_BRIEF_ITEMS).enumerate() {
        let price = s.price.map(|p| format!("@ {p:.2}")).unwrap_or_default();
        lines.push(format!(
            "{}. {} {}（{}）【{}】{}{}",
            i + 1,
            s.side_name,
            s.name,
            s.code,
            s.strategy_name,
            price,
            s.reason_text
                .as_ref()
                .map(|r| format!("\n   {r}"))
                .unwrap_or_default()
        ));
    }
    if items.len() > MAX_BRIEF_ITEMS {
        lines.push(format!("…共 {} 条，完整列表见 quant 看板", items.len()));
    }
    lines.push(String::new());
    lines.push("—— 回复 @机器人 可对信号追问，看板: http://111.170.174.167:8100/".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_formatting() {
        let items = vec![
            SignalItem {
                code: "sh.600000".into(),
                name: "浦发银行".into(),
                strategy_name: "双均线".into(),
                side_name: "买入".into(),
                price: Some(10.5),
                reason_text: Some("5 日均线上穿 20 日均线".into()),
            },
            SignalItem {
                code: "sz.000001".into(),
                name: "平安银行".into(),
                strategy_name: " breakout".into(),
                side_name: "卖出".into(),
                price: None,
                reason_text: None,
            },
        ];
        let text = format_brief(chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(), &items);
        assert!(text.contains("盘后信号简报（2026-07-31）"));
        assert!(text.contains("买入 浦发银行（sh.600000）【双均线】@ 10.50"));
        assert!(text.contains("5 日均线上穿"));
        assert!(text.contains("卖出 平安银行"));
    }

    #[test]
    fn brief_truncates_long_lists() {
        let items: Vec<SignalItem> = (0..30)
            .map(|i| SignalItem {
                code: format!("sh.6000{i:02}"),
                name: "测试".into(),
                strategy_name: "s".into(),
                side_name: "买入".into(),
                price: None,
                reason_text: None,
            })
            .collect();
        let text = format_brief(chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(), &items);
        assert!(text.contains("共 30 条"));
    }
}

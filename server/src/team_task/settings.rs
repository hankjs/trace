//! 团队任务运行时配置：DB 优先、config.toml 兜底。
//!
//! 为什么每次读 DB 而不缓存在 AppState：
//! 这些开关只在「派发一个任务」「推进一次状态机」时读，一个任务全程也就几次，
//! 不是每 token 都读。直接读 DB 换来的是「admin 改完立刻生效」和
//! 「没有缓存失效 bug」，这个取舍很划算。多实例共库时也天然一致。

use super::{role_def, ROLE_DEFS};
use crate::config::Config;
use crate::AppState;
use hank_db::TeamTaskSettings;

/// 取当前生效的运行时配置。
///
/// DB 有值用 DB；DB 没有（首次部署、或还没在 admin 里改过）则用 config.toml
/// 的值作为默认。这样升级上线时行为不变，不需要先去 admin 点一遍。
///
/// 读 DB 失败时**不 bail**，用 config.toml 兜底并 warn——DB 抖一下不该让所有飞书任务失败。
pub async fn effective(state: &AppState) -> TeamTaskSettings {
    effective_with_source(state).await.0
}

/// 与 [`effective`] 相同，额外返回配置来源：`"db"` 或 `"config_file"`。
pub async fn effective_with_source(state: &AppState) -> (TeamTaskSettings, &'static str) {
    match state.db.get_team_task_settings().await {
        Ok(Some(s)) => (s, "db"),
        Ok(None) => (defaults_from_config(&state.config), "config_file"),
        Err(e) => {
            tracing::warn!("读 team_task 运行时配置失败，用 config.toml 兜底: {e:#}");
            (defaults_from_config(&state.config), "config_file")
        }
    }
}

/// 从 config.toml 的两段配置拼出默认值（迁移兜底）。
///
/// DB 优先、config.toml 兜底——升级上线时行为不变。
pub fn defaults_from_config(cfg: &Config) -> TeamTaskSettings {
    TeamTaskSettings {
        task_gate_enabled: cfg.server_agent.task_gate_enabled,
        enabled: cfg.team_task.enabled,
        roles: cfg.team_task.roles.clone(),
        gates: cfg.team_task.gates.clone(),
        max_dev_rounds: cfg.team_task.max_dev_rounds,
        dashboard_base_url: cfg.team_task.dashboard_base_url.clone(),
        updated_by: None,
    }
}

/// 校验一份待写入的配置。返回 Err 时 admin REST 用它的消息回 400。
///
/// 与原先「启动时 bail」的区别：现在是写入时拒绝，用户点保存立刻看到原因，
/// 而不是重启后服务起不来才发现。
///
/// 即使 `enabled = false` 也严格校验——不能让 admin 存一份坏配置进去等着以后炸。
pub fn validate(s: &TeamTaskSettings) -> Result<(), String> {
    if s.enabled && !s.task_gate_enabled {
        return Err("多角色流水线依赖两阶段闸门，请同时开启闸门".to_string());
    }

    if s.enabled && s.roles.is_empty() {
        return Err("至少要配置一个角色".to_string());
    }

    let known_roles: Vec<&str> = ROLE_DEFS.iter().map(|r| r.id).collect();
    let unknown_roles: Vec<&str> = s
        .roles
        .iter()
        .map(String::as_str)
        .filter(|r| role_def(r).is_none())
        .collect();
    if !unknown_roles.is_empty() {
        return Err(format!(
            "未知角色 {:?}，合法值: {}",
            unknown_roles,
            known_roles.join(" / ")
        ));
    }

    // next_role 靠位置查找，重复会让流转错乱
    let mut seen = std::collections::HashSet::new();
    for r in &s.roles {
        if !seen.insert(r.as_str()) {
            return Err(format!("角色列表不能有重复项（重复: {r}）"));
        }
    }

    const KNOWN_GATES: &[&str] = &["dev_start", "review_start", "dev_restart", "test_start"];
    let unknown_gates: Vec<&str> = s
        .gates
        .iter()
        .map(String::as_str)
        .filter(|g| !KNOWN_GATES.contains(g))
        .collect();
    if !unknown_gates.is_empty() {
        return Err(format!(
            "未知闸门边界 {:?}，合法值: {}",
            unknown_gates,
            KNOWN_GATES.join(" / ")
        ));
    }

    // 关闭时也拒绝非法上限——坏配置不该入库
    if s.max_dev_rounds < 1 {
        return Err("最大返工轮次至少为 1".to_string());
    }
    if s.max_dev_rounds > 10 {
        return Err("最大返工轮次不能超过 10（防手滑烧 token）".to_string());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// 单测
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerAgentConfig, ServerConfig, TeamTaskConfig};

    fn minimal_config() -> Config {
        Config {
            server: ServerConfig {
                host: "127.0.0.1".into(),
                port: 3000,
                jwt_secret: "s".into(),
                database_url: "mysql://u:p@localhost/db".into(),
                allowed_dirs: vec![],
                cors_origins: vec![],
                weixin_monitor: false,
                feishu_monitor: false,
                scheduler_enabled: false,
                chrome_path: None,
                admin_base_url: None,
            },
            server_agent: ServerAgentConfig {
                task_gate_enabled: true,
                ..ServerAgentConfig::default()
            },
            quant_a2a: None,
            team_task: TeamTaskConfig {
                enabled: false,
                roles: vec!["developer".into(), "reviewer".into()],
                gates: vec!["dev_start".into()],
                max_dev_rounds: 2,
                dashboard_base_url: Some("http://dash".into()),
            },
        }
    }

    fn valid_settings() -> TeamTaskSettings {
        TeamTaskSettings {
            task_gate_enabled: true,
            enabled: true,
            roles: vec!["developer".into(), "reviewer".into(), "tester".into()],
            gates: vec!["dev_start".into()],
            max_dev_rounds: 3,
            dashboard_base_url: None,
            updated_by: None,
        }
    }

    #[test]
    fn defaults_from_config_maps_both_sections() {
        let cfg = minimal_config();
        let s = defaults_from_config(&cfg);
        assert!(s.task_gate_enabled);
        assert!(!s.enabled);
        assert_eq!(s.roles, vec!["developer", "reviewer"]);
        assert_eq!(s.gates, vec!["dev_start"]);
        assert_eq!(s.max_dev_rounds, 2);
        assert_eq!(s.dashboard_base_url.as_deref(), Some("http://dash"));
        assert!(s.updated_by.is_none());
    }

    #[test]
    fn validate_ok_when_fully_legal() {
        assert!(validate(&valid_settings()).is_ok());
    }

    #[test]
    fn validate_rejects_enabled_without_task_gate() {
        let mut s = valid_settings();
        s.task_gate_enabled = false;
        let err = validate(&s).unwrap_err();
        assert!(err.contains("闸门") || err.contains("依赖"), "{err}");
    }

    #[test]
    fn validate_rejects_empty_roles_when_enabled() {
        let mut s = valid_settings();
        s.roles.clear();
        let err = validate(&s).unwrap_err();
        assert!(err.contains("角色"), "{err}");
    }

    #[test]
    fn validate_rejects_unknown_role() {
        let mut s = valid_settings();
        s.roles = vec!["developer".into(), "designer".into()];
        let err = validate(&s).unwrap_err();
        assert!(
            err.contains("designer") || err.contains("未知角色"),
            "{err}"
        );
        assert!(err.contains("developer"), "{err}");
    }

    #[test]
    fn validate_rejects_duplicate_roles() {
        let mut s = valid_settings();
        s.roles = vec!["developer".into(), "developer".into()];
        let err = validate(&s).unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn validate_rejects_unknown_gate() {
        let mut s = valid_settings();
        s.gates = vec!["dev_start".into(), "final_accept".into()];
        let err = validate(&s).unwrap_err();
        assert!(
            err.contains("final_accept") || err.contains("未知"),
            "{err}"
        );
    }

    #[test]
    fn validate_rejects_max_dev_rounds_below_one() {
        let mut s = valid_settings();
        s.max_dev_rounds = 0;
        assert!(validate(&s).is_err());
    }

    #[test]
    fn validate_rejects_max_dev_rounds_above_ten() {
        let mut s = valid_settings();
        s.max_dev_rounds = 1000;
        let err = validate(&s).unwrap_err();
        assert!(err.contains("10") || err.contains("超过"), "{err}");
    }

    #[test]
    fn validate_strict_even_when_disabled() {
        // 写入时严格：enabled=false 也不能存坏配置
        let mut s = valid_settings();
        s.enabled = false;
        s.task_gate_enabled = false;
        s.roles = vec!["designer".into()];
        s.max_dev_rounds = 0;
        assert!(validate(&s).is_err());
    }

    #[test]
    fn validate_disabled_with_legal_fields_ok() {
        let mut s = valid_settings();
        s.enabled = false;
        s.task_gate_enabled = false; // 关闭流水线时可以关闸门
        assert!(validate(&s).is_ok());
    }
}

use serde_json::Value;
use std::path::{Path, PathBuf};

/// 危险命令黑名单的单一来源（FR-PERM-3）。
/// shell.rs 与 PermissionGuard 共用此列表，避免两份黑名单漂移。
/// 采用子串匹配（大小写无关）；这是粗粒度防线，不替代 sandbox 与权限模式。
pub const DEFAULT_BLOCKED_COMMANDS: &[&str] = &[
    "rm -rf /",
    "mkfs",
    "dd if=/dev",
    ":(){ :|:& };:",
    "chmod -R 777 /",
    "curl | sh",
    "wget | sh",
    "shutdown",
    "reboot",
    "halt",
    "poweroff",
];

/// 工具风险等级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    /// 只读操作，无副作用
    Safe,
    /// 文件写入等可逆操作
    Moderate,
    /// Shell 执行、危险 git 操作等
    Dangerous,
}

/// 权限模式（对齐 Codex sandbox 三档与 Claude permission-mode，FR-PERM-1）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    /// 仅探查：只允许只读工具
    ReadOnly,
    /// 可写根内编辑：写工具放行，shell/危险操作需审批
    WorkspaceWrite,
    /// 单次命令/工具经批准后执行
    Escalated,
    /// 仅受信自动化环境显式启用：全部放行（黑名单仍生效）
    Unrestricted,
}

impl Default for PermissionMode {
    fn default() -> Self {
        Self::WorkspaceWrite
    }
}

impl PermissionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::Escalated => "escalated",
            Self::Unrestricted => "unrestricted",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "read-only" | "read_only" | "readonly" => Self::ReadOnly,
            "escalated" => Self::Escalated,
            "unrestricted" => Self::Unrestricted,
            _ => Self::WorkspaceWrite,
        }
    }
}

/// 权限检查结果
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// 允许执行
    Allow,
    /// 拒绝执行，附带原因
    Deny(String),
    /// 需要用户确认
    NeedApproval(String),
}

/// 权限配置
#[derive(Debug, Clone)]
pub struct PermissionConfig {
    /// 权限模式
    pub mode: PermissionMode,
    /// 允许写入的路径前缀
    pub sandbox_paths: Vec<String>,
    /// Shell 额外黑名单命令
    pub blocked_commands: Vec<String>,
    /// 禁止写入的路径前缀；相对路径以 work_dir 为根解析。
    pub blocked_paths: Vec<String>,
    /// 只读文件工具也必须留在 sandbox 内。server Agent 开启，普通会话默认关闭。
    pub restrict_read_paths: bool,
    /// 自动放行的工具名
    pub auto_approve_tools: Vec<String>,
    /// 预授权的命令前缀（如 "npm test"、"cargo test"，FR-PERM-8）
    pub approved_prefixes: Vec<String>,
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            mode: PermissionMode::default(),
            sandbox_paths: Vec::new(),
            blocked_commands: DEFAULT_BLOCKED_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            blocked_paths: Vec::new(),
            restrict_read_paths: false,
            auto_approve_tools: vec![
                "read_file".to_string(),
                "search".to_string(),
                "list_directory".to_string(),
            ],
            approved_prefixes: Vec::new(),
        }
    }
}

/// 权限守卫
pub struct PermissionGuard {
    config: PermissionConfig,
}

impl PermissionGuard {
    pub fn new(config: PermissionConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(PermissionConfig::default())
    }

    /// 以指定权限模式构建
    pub fn with_mode(mode: PermissionMode) -> Self {
        let mut config = PermissionConfig::default();
        config.mode = mode;
        Self::new(config)
    }

    pub fn mode(&self) -> PermissionMode {
        self.config.mode
    }

    /// 词法归一化路径：解析 `.`/`..`/重复分隔符，不触碰文件系统。
    /// 用于 sandbox 前缀校验，防止 `a/../../etc` 这类穿越绕过子串检测。
    fn normalize_path(path: &str) -> String {
        let is_absolute = path.starts_with('/');
        let mut parts: Vec<&str> = Vec::new();
        for component in path.split('/') {
            match component {
                "" | "." => {}
                ".." => {
                    // 弹出上一级；绝对路径不能越过根
                    parts.pop();
                }
                c => parts.push(c),
            }
        }
        if is_absolute {
            format!("/{}", parts.join("/"))
        } else {
            parts.join("/")
        }
    }

    /// 解析已存在的最长前缀，避免 worktree 内符号链接把读写路径带到 sandbox 外。
    /// 文件尚不存在时保留尾部组件，供 write_file 的父目录校验使用。
    fn canonicalize_with_missing(path: &str) -> String {
        let mut existing = PathBuf::from(path);
        let mut missing = Vec::new();
        while !existing.exists() {
            let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
                break;
            };
            missing.push(name);
            if !existing.pop() {
                break;
            }
        }
        let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
        for component in missing.into_iter().rev() {
            resolved.push(component);
        }
        Self::normalize_path(&resolved.to_string_lossy())
    }

    /// 检查写路径是否落在 sandbox 内（FR-PERM-4）。
    /// sandbox_paths 为空时回退到 work_dir 前缀。
    /// 先词法归一化再做前缀匹配，并要求边界对齐（避免 /workspace-evil 命中 /workspace）。
    fn path_in_sandbox(&self, path: &str, work_dir: &str) -> bool {
        let joined = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{}", work_dir.trim_end_matches('/'), path)
        };
        let resolved = Self::canonicalize_with_missing(&joined);
        let roots: Vec<String> = if self.config.sandbox_paths.is_empty() {
            if work_dir.is_empty() {
                return true; // 未配置 work_dir 时不做路径限制
            }
            vec![work_dir.trim_end_matches('/').to_string()]
        } else {
            self.config.sandbox_paths.clone()
        };
        roots.iter().any(|prefix| {
            let norm_prefix = std::fs::canonicalize(Path::new(prefix.trim_end_matches('/')))
                .map(|path| Self::normalize_path(&path.to_string_lossy()))
                .unwrap_or_else(|_| Self::normalize_path(prefix.trim_end_matches('/')));
            if norm_prefix.is_empty() {
                return true;
            }
            // 前缀匹配 + 边界对齐：完全相等，或紧跟 '/' 分隔
            resolved.starts_with(&norm_prefix)
                && (resolved.len() == norm_prefix.len()
                    || resolved.as_bytes().get(norm_prefix.len()) == Some(&b'/'))
        })
    }

    fn path_is_blocked(&self, path: &str, work_dir: &str) -> bool {
        let joined = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{}", work_dir.trim_end_matches('/'), path)
        };
        let resolved = Self::canonicalize_with_missing(&joined);
        self.config.blocked_paths.iter().any(|blocked| {
            let prefix = if blocked.starts_with('/') {
                blocked.clone()
            } else {
                format!("{}/{}", work_dir.trim_end_matches('/'), blocked)
            };
            let prefix = Self::canonicalize_with_missing(prefix.trim_end_matches('/'));
            resolved == prefix || resolved.starts_with(&format!("{prefix}/"))
        })
    }

    /// 检查命令是否命中预授权前缀（FR-PERM-8）
    fn matches_approved_prefix(&self, command: &str) -> bool {
        let trimmed = command.trim();
        self.config
            .approved_prefixes
            .iter()
            .any(|p| trimmed.starts_with(p.trim()))
    }

    /// 检查工具执行权限
    pub fn check(
        &self,
        tool_name: &str,
        input: &Value,
        risk: ToolRisk,
        work_dir: &str,
    ) -> PermissionDecision {
        // 1. 文件工具先做路径校验；server Agent 的只读工具也不能越出 worktree。
        let path_tool = matches!(
            tool_name,
            "read_file" | "write_file" | "str_replace" | "list_directory" | "search"
        );
        if path_tool {
            let path = input["path"].as_str().unwrap_or(".");
            if self.path_is_blocked(path, work_dir) {
                return PermissionDecision::Deny(format!(
                    "Path '{path}' is inside a blocked workspace path"
                ));
            }
            let is_write = matches!(tool_name, "write_file" | "str_replace");
            if (is_write || self.config.restrict_read_paths)
                && !self.path_in_sandbox(path, work_dir)
            {
                return PermissionDecision::Deny(format!(
                    "Path '{path}' is outside allowed sandbox/workspace roots"
                ));
            }
        }

        // 2. Safe 工具与自动放行工具直接放行（任何模式）
        if risk == ToolRisk::Safe
            || self
                .config
                .auto_approve_tools
                .contains(&tool_name.to_string())
        {
            return PermissionDecision::Allow;
        }

        // 3. Shell 黑名单优先于一切：命中直接 Deny（即使 unrestricted）
        if tool_name == "shell" {
            if let Some(cmd) = input["command"].as_str() {
                let lower = cmd.to_lowercase();
                for blocked in &self.config.blocked_commands {
                    if lower.contains(&blocked.to_lowercase()) {
                        return PermissionDecision::Deny(format!(
                            "Command contains blocked pattern: '{blocked}'"
                        ));
                    }
                }
            }
        }

        // 4. ReadOnly 模式：拒绝所有非只读工具
        if self.config.mode == PermissionMode::ReadOnly {
            return PermissionDecision::Deny(format!(
                "Tool '{tool_name}' is not allowed in read-only mode"
            ));
        }

        // 5. Unrestricted 模式：黑名单外全部放行
        if self.config.mode == PermissionMode::Unrestricted {
            return PermissionDecision::Allow;
        }

        // 6. 预授权命令前缀放行（FR-PERM-8）
        if tool_name == "shell" {
            if let Some(cmd) = input["command"].as_str() {
                if self.matches_approved_prefix(cmd) {
                    return PermissionDecision::Allow;
                }
            }
        }

        // 7. Escalated 模式：Dangerous 工具需要单次审批
        if self.config.mode == PermissionMode::Escalated && risk == ToolRisk::Dangerous {
            let reason = match tool_name {
                "shell" => {
                    let cmd = input["command"].as_str().unwrap_or("<unknown>");
                    let preview: String = cmd.chars().take(100).collect();
                    format!("Shell command execution: {}", preview)
                }
                "git" => {
                    let args = input["args"].as_str().unwrap_or("<unknown>");
                    format!("Git operation: {}", args)
                }
                _ => format!("Dangerous tool: {tool_name}"),
            };
            return PermissionDecision::NeedApproval(reason);
        }

        // 8. workspace-write 模式：Moderate/Dangerous 工具在通过 sandbox/黑名单后放行。
        //    （workspace-write 授予工作区内自主执行能力，对齐 Codex sandbox 语义）
        PermissionDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn guard(mode: PermissionMode) -> PermissionGuard {
        let mut cfg = PermissionConfig::default();
        cfg.mode = mode;
        cfg.sandbox_paths = vec!["/work".to_string()];
        PermissionGuard::new(cfg)
    }

    #[test]
    fn test_blocked_command_always_denied() {
        // 危险命令在任何模式下都被拒绝（FR-PERM-3）
        for mode in [PermissionMode::WorkspaceWrite, PermissionMode::Unrestricted] {
            let g = guard(mode);
            let d = g.check(
                "shell",
                &json!({"command": "rm -rf /"}),
                ToolRisk::Dangerous,
                "/work",
            );
            assert!(matches!(d, PermissionDecision::Deny(_)), "mode {:?}", mode);
        }
    }

    #[test]
    fn test_read_only_denies_writes() {
        let g = guard(PermissionMode::ReadOnly);
        let d = g.check(
            "write_file",
            &json!({"path": "a.txt"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Deny(_)));
        // 只读工具仍然放行
        let d2 = g.check(
            "read_file",
            &json!({"path": "a.txt"}),
            ToolRisk::Safe,
            "/work",
        );
        assert!(matches!(d2, PermissionDecision::Allow));
    }

    #[test]
    fn test_write_outside_sandbox_denied() {
        let g = guard(PermissionMode::WorkspaceWrite);
        let d = g.check(
            "write_file",
            &json!({"path": "/etc/passwd"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Deny(_)));
        // sandbox 内允许
        let d2 = g.check(
            "write_file",
            &json!({"path": "/work/a.txt"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d2, PermissionDecision::Allow));
    }

    #[test]
    fn test_path_traversal_denied() {
        let g = guard(PermissionMode::WorkspaceWrite);
        let d = g.check(
            "write_file",
            &json!({"path": "../../etc/passwd"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_blocked_path_denies_direct_and_traversal_writes() {
        let mut cfg = PermissionConfig::default();
        cfg.sandbox_paths = vec!["/work".to_string()];
        cfg.blocked_paths = vec!["client".to_string()];
        let guard = PermissionGuard::new(cfg);
        for path in ["client/src/App.vue", "server/../client/package.json"] {
            let decision = guard.check(
                "write_file",
                &json!({"path": path}),
                ToolRisk::Moderate,
                "/work",
            );
            assert!(matches!(decision, PermissionDecision::Deny(_)), "{path}");
        }
    }

    #[test]
    fn test_restricted_read_path_cannot_escape_workspace() {
        let mut cfg = PermissionConfig::default();
        cfg.sandbox_paths = vec!["/work".to_string()];
        cfg.restrict_read_paths = true;
        let guard = PermissionGuard::new(cfg);
        for (tool, path) in [
            ("read_file", "/opt/hank/config.toml"),
            ("list_directory", "../"),
            ("search", "/etc"),
        ] {
            let decision = guard.check(tool, &json!({"path": path}), ToolRisk::Safe, "/work");
            assert!(
                matches!(decision, PermissionDecision::Deny(_)),
                "{tool}: {path}"
            );
        }
    }

    #[test]
    fn test_path_traversal_midpath_denied() {
        // 中段 .. 穿越（不以 /../ 结尾）应被归一化后拦截
        let g = guard(PermissionMode::WorkspaceWrite);
        let d = g.check(
            "write_file",
            &json!({"path": "sub/../../../etc/passwd"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_path_traversal_back_into_sandbox_allowed() {
        // 先出后回、最终仍落在 sandbox 内应放行
        let g = guard(PermissionMode::WorkspaceWrite);
        let d = g.check(
            "write_file",
            &json!({"path": "sub/../a.txt"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Allow));
    }

    #[test]
    fn test_sibling_prefix_not_confused() {
        // /work-evil 不应被 /work 前缀误匹配（边界对齐）
        let mut cfg = PermissionConfig::default();
        cfg.mode = PermissionMode::WorkspaceWrite;
        cfg.sandbox_paths = vec!["/work".to_string()];
        let g = PermissionGuard::new(cfg);
        let d = g.check(
            "write_file",
            &json!({"path": "/work-evil/a.txt"}),
            ToolRisk::Moderate,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Deny(_)));
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_cannot_escape_sandbox() {
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("trace-permission-{}-{nonce}", std::process::id()));
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::create_dir_all(&outside).expect("create outside directory");
        std::fs::write(outside.join("secret.txt"), "secret").expect("create outside file");
        symlink(&outside, workspace.join("escape")).expect("create symlink");

        let mut cfg = PermissionConfig::default();
        cfg.mode = PermissionMode::WorkspaceWrite;
        cfg.sandbox_paths = vec![workspace.to_string_lossy().into_owned()];
        cfg.restrict_read_paths = true;
        let guard = PermissionGuard::new(cfg);
        let decision = guard.check(
            "read_file",
            &json!({"path": "escape/secret.txt"}),
            ToolRisk::Safe,
            &workspace.to_string_lossy(),
        );

        std::fs::remove_dir_all(&root).expect("clean temporary directory");
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_cannot_enter_blocked_path() {
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let workspace =
            std::env::temp_dir().join(format!("trace-blocked-path-{}-{nonce}", std::process::id()));
        let client = workspace.join("client");
        std::fs::create_dir_all(&client).expect("create blocked directory");
        symlink(&client, workspace.join("alias")).expect("create symlink");

        let mut cfg = PermissionConfig::default();
        cfg.mode = PermissionMode::WorkspaceWrite;
        cfg.sandbox_paths = vec![workspace.to_string_lossy().into_owned()];
        cfg.blocked_paths = vec!["client".to_string()];
        let guard = PermissionGuard::new(cfg);
        let decision = guard.check(
            "write_file",
            &json!({"path": "alias/App.vue"}),
            ToolRisk::Moderate,
            &workspace.to_string_lossy(),
        );

        std::fs::remove_dir_all(&workspace).expect("clean temporary directory");
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    #[test]
    fn test_dangerous_needs_approval_in_escalated() {
        let g = guard(PermissionMode::Escalated);
        let d = g.check(
            "shell",
            &json!({"command": "ls -la"}),
            ToolRisk::Dangerous,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::NeedApproval(_)));
    }

    #[test]
    fn test_workspace_write_allows_shell() {
        // workspace-write 模式下普通 shell 自主执行（对齐 Codex sandbox 语义）
        let g = guard(PermissionMode::WorkspaceWrite);
        let d = g.check(
            "shell",
            &json!({"command": "ls -la"}),
            ToolRisk::Dangerous,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Allow));
    }

    #[test]
    fn test_approved_prefix_allows_shell() {
        let mut cfg = PermissionConfig::default();
        cfg.mode = PermissionMode::WorkspaceWrite;
        cfg.approved_prefixes = vec!["cargo test".to_string()];
        let g = PermissionGuard::new(cfg);
        let d = g.check(
            "shell",
            &json!({"command": "cargo test --workspace"}),
            ToolRisk::Dangerous,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Allow));
    }

    #[test]
    fn test_unrestricted_allows_normal_shell() {
        let g = guard(PermissionMode::Unrestricted);
        let d = g.check(
            "shell",
            &json!({"command": "ls -la"}),
            ToolRisk::Dangerous,
            "/work",
        );
        assert!(matches!(d, PermissionDecision::Allow));
    }
}

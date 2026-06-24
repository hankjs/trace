/// Represents a segment of a system prompt that can be static or dynamically generated
pub enum PromptSegment {
    /// Static text that doesn't change
    Static(&'static str),
    /// Dynamically generated text
    Dynamic(String),
    /// Conditional segment — only included if condition is true
    Conditional { content: String, condition: bool },
}

/// 自研 base 提示词（基于公开可见运行模式，不复制任何隐藏 system prompt）。
/// 该段保持稳定以利于 prompt cache 复用（FR-CTX-7）。
pub const BASE_CODING_PROMPT: &str = "You are a coding agent working in the user's repository.\n\
Operate pragmatically:\n\
- inspect before editing\n\
- prefer existing project patterns\n\
- keep changes scoped\n\
- use structured file edits (str_replace/write_file), not shell redirection\n\
- run relevant tests when permitted\n\
- recover from failures by reading errors and making targeted fixes\n\
- never revert unrelated user changes; no git reset --hard unless asked\n\
- summarize changed files, verification, and remaining risk";

/// 工具目录条目（用于 developer/runtime 段注入，FR-CTX-1）
#[derive(Debug, Clone)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub risk: String,
}

/// 技能索引条目（渐进式披露，默认只放 name/description/path，FR-CTX-8）
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: String,
}

/// developer/runtime 段上下文：权限、能力目录、技能索引（FR-CTX-1）
#[derive(Debug, Clone, Default)]
pub struct RuntimeContext {
    pub permission_mode: String,
    pub approval_policy: String,
    pub writable_roots: Vec<String>,
    pub network_policy: String,
    pub tools: Vec<ToolInfo>,
    pub skills: Vec<SkillInfo>,
}

impl RuntimeContext {
    /// 渲染为 developer/runtime 段文本
    pub fn render(&self) -> String {
        let mut out = String::from("[developer/runtime]\n");
        out.push_str(&format!("- permission mode: {}\n", self.permission_mode));
        out.push_str(&format!("- approval policy: {}\n", self.approval_policy));
        let roots = if self.writable_roots.is_empty() {
            "<none>".to_string()
        } else {
            self.writable_roots.join(", ")
        };
        out.push_str(&format!("- writable roots: {}\n", roots));
        out.push_str(&format!("- network policy: {}\n", self.network_policy));
        if !self.tools.is_empty() {
            out.push_str("- available tools:\n");
            for t in &self.tools {
                out.push_str(&format!("  - {} [{}]: {}\n", t.name, t.risk, t.description));
            }
        }
        if !self.skills.is_empty() {
            out.push_str("- skills index:\n");
            for s in &self.skills {
                out.push_str(&format!("  - {} ({}): {}\n", s.name, s.path, s.description));
            }
        }
        out.trim_end().to_string()
    }
}

/// environment 段上下文：cwd/shell/date/timezone/sandbox（FR-CTX-2）
#[derive(Debug, Clone, Default)]
pub struct EnvironmentContext {
    pub cwd: Option<String>,
    pub shell: String,
    pub current_date: String,
    pub timezone: String,
    pub repo_root: Option<String>,
    pub sandbox_mode: String,
    pub network_policy: String,
}

impl EnvironmentContext {
    /// 渲染为 <environment_context> 块文本
    pub fn render(&self) -> String {
        let mut out = String::from("<environment_context>\n");
        if let Some(ref cwd) = self.cwd {
            out.push_str(&format!("  <cwd>{}</cwd>\n", cwd));
        }
        out.push_str(&format!("  <shell>{}</shell>\n", self.shell));
        out.push_str(&format!("  <current_date>{}</current_date>\n", self.current_date));
        out.push_str(&format!("  <timezone>{}</timezone>\n", self.timezone));
        if let Some(ref root) = self.repo_root {
            out.push_str(&format!("  <repo_root>{}</repo_root>\n", root));
        }
        out.push_str(&format!("  <sandbox_mode>{}</sandbox_mode>\n", self.sandbox_mode));
        out.push_str(&format!("  <network_policy>{}</network_policy>\n", self.network_policy));
        out.push_str("</environment_context>");
        out
    }
}

/// Build a system prompt from multiple segments joined with double newlines
pub fn build_system_prompt(segments: &[PromptSegment]) -> String {
    segments
        .iter()
        .filter_map(|seg| match seg {
            PromptSegment::Static(s) => Some(s.to_string()),
            PromptSegment::Dynamic(s) => Some(s.clone()),
            PromptSegment::Conditional { content, condition } => {
                if *condition { Some(content.clone()) } else { None }
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 一段已命名的提示词层（用于分层组装与 debug 摘要）
pub struct NamedSegment {
    pub name: &'static str,
    pub content: String,
}

/// 按 base/developer/environment/project/user 分层组装系统提示词（FR-CTX-1）。
///
/// - `base`: 稳定基础指令（缺省使用 BASE_CODING_PROMPT）
/// - `runtime`: 权限 + 工具目录 + 技能索引
/// - `environment`: 环境上下文块
/// - `project`: 项目记忆文件（CLAUDE.md/AGENTS.md/.cursorrules）
///
/// 注意：用户任务不在此拼接，作为独立 user message 发送（FR-CTX-7 / 第8节约束3）。
/// 返回组装后的 system prompt 文本 + 命名分层（供 ContextAssembled debug 摘要使用）。
pub fn build_layered_prompt(
    base: Option<&str>,
    runtime: Option<&RuntimeContext>,
    environment: Option<&EnvironmentContext>,
    project_segments: &[PromptSegment],
) -> (String, Vec<NamedSegment>) {
    let mut named: Vec<NamedSegment> = Vec::new();

    named.push(NamedSegment {
        name: "base",
        content: base.unwrap_or(BASE_CODING_PROMPT).to_string(),
    });

    if let Some(rt) = runtime {
        named.push(NamedSegment { name: "developer", content: rt.render() });
    }

    if let Some(env) = environment {
        named.push(NamedSegment { name: "environment", content: env.render() });
    }

    let project_text = build_system_prompt(project_segments);
    if !project_text.is_empty() {
        named.push(NamedSegment { name: "project", content: project_text });
    }

    let assembled = named
        .iter()
        .map(|s| s.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    (assembled, named)
}

/// 扫描项目目录，发现并加载上下文文件
pub async fn discover_project_context(work_dir: &str) -> Vec<PromptSegment> {
    let mut segments = Vec::new();
    let context_files = [
        ("CLAUDE.md", "Project Instructions (CLAUDE.md)"),
        ("AGENTS.md", "Agent Instructions (AGENTS.md)"),
        (".cursorrules", "Project Rules (.cursorrules)"),
    ];

    let max_chars: usize = 4000;

    for (filename, label) in &context_files {
        let path = format!("{}/{}", work_dir.trim_end_matches('/'), filename);
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            let truncated = if content.chars().count() > max_chars {
                let end = content.char_indices().nth(max_chars).map_or(content.len(), |(i, _)| i);
                format!("{}...\n[truncated at {} chars]", &content[..end], max_chars)
            } else {
                content
            };
            segments.push(PromptSegment::Dynamic(format!(
                "# {label}\n\n{truncated}"
            )));
        }
    }

    segments
}

/// Pre-built prompt segments for common scenarios
pub mod segments {
    /// Generate a budget warning segment
    pub fn budget_warning(percent: u8) -> String {
        format!(
            "⚠️ Context window usage at {}%. Be concise with responses.",
            percent
        )
    }

    /// Generate a loop warning segment
    pub fn loop_warning(tool_name: &str, count: usize) -> String {
        format!(
            "⚠️ Loop detected: tool '{}' called {} times in succession. Vary your approach or use different tools.",
            tool_name, count
        )
    }

    /// Static hint about think mode
    pub fn think_mode_hint() -> &'static str {
        "You are in THINK mode. Analyze the situation and plan your next steps. \
         Do NOT use tools. Just reason about what to do next."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_prompt_mixed() {
        let segments = [
            PromptSegment::Static("Base instructions"),
            PromptSegment::Dynamic("Dynamic context".to_string()),
            PromptSegment::Static("Final guidelines"),
        ];

        let prompt = build_system_prompt(&segments);
        assert!(prompt.contains("Base instructions"));
        assert!(prompt.contains("Dynamic context"));
        assert!(prompt.contains("Final guidelines"));
        assert!(prompt.contains("\n\n"));
    }

    #[test]
    fn test_budget_warning_segment() {
        let warning = segments::budget_warning(85);
        assert!(warning.contains("85"));
        assert!(warning.contains("Context window"));
    }

    #[test]
    fn test_loop_warning_segment() {
        let warning = segments::loop_warning("shell", 4);
        assert!(warning.contains("shell"));
        assert!(warning.contains("4"));
    }

    #[test]
    fn test_layered_prompt_stable_and_ordered() {
        let rt = RuntimeContext {
            permission_mode: "workspace-write".to_string(),
            approval_policy: "auto".to_string(),
            writable_roots: vec!["/work".to_string()],
            network_policy: "restricted".to_string(),
            tools: vec![ToolInfo {
                name: "read_file".to_string(),
                description: "read a file".to_string(),
                risk: "safe".to_string(),
            }],
            skills: vec![],
        };
        let env = EnvironmentContext {
            cwd: Some("/work".to_string()),
            shell: "/bin/zsh".to_string(),
            current_date: "2026-06-04".to_string(),
            timezone: "UTC".to_string(),
            repo_root: Some("/work".to_string()),
            sandbox_mode: "workspace-write".to_string(),
            network_policy: "restricted".to_string(),
        };
        let project = [PromptSegment::Dynamic("# Project\nrules".to_string())];

        let (prompt, named) = build_layered_prompt(None, Some(&rt), Some(&env), &project);
        // 分层顺序：base -> developer -> environment -> project
        assert_eq!(named[0].name, "base");
        assert_eq!(named[1].name, "developer");
        assert_eq!(named[2].name, "environment");
        assert_eq!(named[3].name, "project");
        assert!(prompt.contains("coding agent"));
        assert!(prompt.contains("permission mode: workspace-write"));
        assert!(prompt.contains("<environment_context>"));
        assert!(prompt.contains("read_file"));
        assert!(prompt.contains("# Project"));

        // 相同输入应稳定输出
        let (prompt2, _) = build_layered_prompt(None, Some(&rt), Some(&env), &project);
        assert_eq!(prompt, prompt2);
    }

    #[test]
    fn test_layered_prompt_missing_project_ok() {
        // 缺失项目上下文不报错（FR-CTX-3）
        let (prompt, named) = build_layered_prompt(None, None, None, &[]);
        assert_eq!(named.len(), 1);
        assert_eq!(named[0].name, "base");
        assert!(!prompt.is_empty());
    }
}

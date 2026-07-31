use crate::{Tool, ToolOutput, ToolRisk, LONG_TOOL_TIMEOUT};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;
use tracing::warn;

/// Git 子命令白名单
const ALLOWED_COMMANDS: &[&str] = &[
    "status",
    "diff",
    "log",
    "commit",
    "add",
    "branch",
    "checkout",
    "stash",
    "reset",
    "show",
    "blame",
    "tag",
    "merge",
    "rebase",
    "cherry-pick",
    "fetch",
    "pull",
    "rev-parse",
    "config",
];

/// 需要确认的危险操作
const DANGEROUS_PATTERNS: &[&str] = &[
    "push --force",
    "push -f",
    "reset --hard",
    "clean -f",
    "branch -D",
];

pub struct GitTool {
    work_dir: Option<String>,
    run_as_user: Option<String>,
}

impl GitTool {
    pub fn new(work_dir: Option<String>) -> Self {
        Self {
            work_dir,
            run_as_user: None,
        }
    }

    pub fn new_as_user(work_dir: Option<String>, user: impl Into<String>) -> Self {
        Self {
            work_dir,
            run_as_user: Some(user.into()),
        }
    }

    fn parse_subcommand(args: &str) -> Option<&str> {
        args.split_whitespace().next()
    }

    fn is_allowed(subcommand: &str) -> bool {
        ALLOWED_COMMANDS.contains(&subcommand)
    }

    fn is_dangerous(args: &str) -> bool {
        let lower = args.to_lowercase();
        DANGEROUS_PATTERNS.iter().any(|p| lower.contains(p))
    }

    #[allow(dead_code)]
    fn is_write_command(subcommand: &str) -> bool {
        matches!(
            subcommand,
            "commit"
                | "checkout"
                | "stash"
                | "add"
                | "reset"
                | "merge"
                | "rebase"
                | "cherry-pick"
                | "tag"
        )
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        "Execute git commands. Supports: status, diff, log, commit, add, branch, checkout, \
         stash, reset, show, blame, tag, merge, rebase, cherry-pick, fetch, pull, rev-parse, config. \
         Dangerous operations (push --force, reset --hard) are blocked."
    }

    fn timeout(&self) -> Duration {
        LONG_TOOL_TIMEOUT
    }

    fn is_write(&self) -> bool {
        // 保守策略: 默认 true，因为 git 操作可能修改工作区
        true
    }

    fn risk_level(&self) -> ToolRisk {
        ToolRisk::Moderate
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "args": {
                    "type": "string",
                    "description": "Git arguments (e.g. 'status', 'diff --staged', 'commit -m \"msg\"', 'log --oneline -10')"
                }
            },
            "required": ["args"]
        })
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let args = input["args"].as_str().unwrap_or_default().to_string();

        if args.is_empty() {
            return Ok(ToolOutput {
                content: "Error: args is required".to_string(),
                is_error: true,
            });
        }

        // 解析子命令
        let subcommand = match Self::parse_subcommand(&args) {
            Some(cmd) => cmd.to_string(),
            None => {
                return Ok(ToolOutput {
                    content: "Error: could not parse git subcommand".to_string(),
                    is_error: true,
                });
            }
        };

        // 白名单检查
        if !Self::is_allowed(&subcommand) {
            return Ok(ToolOutput {
                content: format!(
                    "Error: git subcommand '{}' is not allowed. Allowed: {:?}",
                    subcommand, ALLOWED_COMMANDS
                ),
                is_error: true,
            });
        }

        // 危险操作检查
        if Self::is_dangerous(&args) {
            warn!("Blocked dangerous git command: git {args}");
            return Ok(ToolOutput {
                content: format!(
                    "Error: 'git {args}' is a dangerous operation and has been blocked. \
                     If you need to perform this action, ask the user for confirmation first using ask_user."
                ),
                is_error: true,
            });
        }

        let Some(parsed_args) = shlex::split(&args) else {
            return Ok(ToolOutput {
                content: "Error: could not parse git arguments".to_string(),
                is_error: true,
            });
        };
        let mut shell_cmd = if let Some(user) = &self.run_as_user {
            let mut cmd = Command::new("sudo");
            let home = format!("HOME=/home/{user}");
            let path = format!(
                "PATH=/home/{user}/.cargo/bin:/home/{user}/.local/bin:/usr/local/bin:/usr/bin:/bin"
            );
            cmd.args([
                "--non-interactive",
                "--user",
                user,
                "--",
                "env",
                &home,
                &path,
                "sh",
                "-c",
                "umask 0007; exec \"$@\"",
                "sh",
                "git",
            ]);
            cmd
        } else {
            Command::new("git")
        };
        shell_cmd.args(&parsed_args);
        if let Some(ref dir) = self.work_dir {
            shell_cmd.current_dir(dir);
        }

        let result = tokio::time::timeout(LONG_TOOL_TIMEOUT, shell_cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);

                let combined = if stderr.is_empty() || code == 0 {
                    // git 有些正常输出走 stderr (如 progress)
                    if stdout.is_empty() && !stderr.is_empty() {
                        stderr.to_string()
                    } else {
                        stdout.to_string()
                    }
                } else {
                    format!("STDOUT:\n{stdout}\nSTDERR:\n{stderr}")
                };

                // 截断过长输出
                let content = if combined.len() > 100_000 {
                    let half = 50_000;
                    format!(
                        "{}\n\n... [output truncated, {} bytes total] ...\n\n{}",
                        &combined[..half],
                        combined.len(),
                        &combined[combined.len() - half..]
                    )
                } else {
                    combined
                };

                Ok(ToolOutput {
                    content,
                    is_error: code != 0,
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                content: format!("Error executing git: {e}"),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: "Error: git command timed out".to_string(),
                is_error: true,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GitTool;

    #[test]
    fn shell_control_operators_are_arguments_not_commands() {
        let parsed = shlex::split("status && cat /opt/hank/config.toml").unwrap();
        assert_eq!(parsed[0], "status");
        assert_eq!(parsed[1], "&&");
        assert!(GitTool::is_allowed(&parsed[0]));
    }
}

use crate::{Tool, ToolOutput, LONG_TOOL_TIMEOUT};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// 自动检测的测试框架
#[derive(Debug, Clone, PartialEq)]
enum TestFramework {
    Cargo,
    Jest,
    Pytest,
    Go,
    Unknown,
}

pub struct TestRunnerTool {
    work_dir: Option<String>,
    run_as_user: Option<String>,
}

impl TestRunnerTool {
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

    fn resolve_dir(&self) -> String {
        self.work_dir.clone().unwrap_or_else(|| ".".to_string())
    }

    /// 自动检测项目使用的测试框架
    async fn detect_framework(&self, dir: &str) -> TestFramework {
        let dir_path = Path::new(dir);

        if dir_path.join("Cargo.toml").exists() {
            return TestFramework::Cargo;
        }
        if dir_path.join("package.json").exists() {
            // 检查是否有 jest 配置
            if dir_path.join("jest.config.js").exists() || dir_path.join("jest.config.ts").exists()
            {
                return TestFramework::Jest;
            }
            // 检查 package.json 中是否有 jest
            if let Ok(content) = tokio::fs::read_to_string(dir_path.join("package.json")).await {
                if content.contains("\"jest\"") || content.contains("\"vitest\"") {
                    return TestFramework::Jest;
                }
            }
        }
        if dir_path.join("go.mod").exists() {
            return TestFramework::Go;
        }
        if dir_path.join("pytest.ini").exists()
            || dir_path.join("pyproject.toml").exists()
            || dir_path.join("setup.py").exists()
        {
            return TestFramework::Pytest;
        }

        TestFramework::Unknown
    }

    fn build_command(
        &self,
        framework: &TestFramework,
        scope: &str,
        target: &str,
        _dir: &str,
    ) -> Option<(String, Vec<String>)> {
        match framework {
            TestFramework::Cargo => {
                let mut args = vec!["test".to_string()];
                match scope {
                    "file" | "function" if !target.is_empty() => {
                        args.push(target.to_string());
                    }
                    _ => {}
                }
                args.push("--".to_string());
                args.push("--nocapture".to_string());
                Some(("cargo".to_string(), args))
            }
            TestFramework::Jest => {
                let mut args = vec!["jest".to_string()];
                match scope {
                    "file" if !target.is_empty() => {
                        args.push(target.to_string());
                    }
                    "function" if !target.is_empty() => {
                        args.push("-t".to_string());
                        args.push(target.to_string());
                    }
                    _ => {}
                }
                args.push("--no-coverage".to_string());
                Some(("npx".to_string(), args))
            }
            TestFramework::Pytest => {
                let mut args = Vec::new();
                match scope {
                    "file" if !target.is_empty() => {
                        args.push(target.to_string());
                    }
                    "function" if !target.is_empty() => {
                        args.push("-k".to_string());
                        args.push(target.to_string());
                    }
                    _ => {}
                }
                args.push("-v".to_string());
                Some(("pytest".to_string(), args))
            }
            TestFramework::Go => {
                let mut args = vec!["test".to_string()];
                match scope {
                    "file" if !target.is_empty() => {
                        args.push(target.to_string());
                    }
                    "function" if !target.is_empty() => {
                        args.push("-run".to_string());
                        args.push(target.to_string());
                    }
                    "all" | _ => {
                        args.push("./...".to_string());
                    }
                }
                args.push("-v".to_string());
                Some(("go".to_string(), args))
            }
            TestFramework::Unknown => None,
        }
    }
}

#[async_trait]
impl Tool for TestRunnerTool {
    fn name(&self) -> &str {
        "test_runner"
    }

    fn description(&self) -> &str {
        "Run tests for the project. Auto-detects the test framework (cargo test, jest, pytest, go test). \
         Supports running all tests, a specific file, or a specific test function."
    }

    fn timeout(&self) -> Duration {
        LONG_TOOL_TIMEOUT
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["all", "file", "function"],
                    "description": "Test scope: 'all' runs everything, 'file' runs a specific file, 'function' runs a specific test"
                },
                "target": {
                    "type": "string",
                    "description": "Target file path or test function name (required for file/function scope)"
                },
                "framework": {
                    "type": "string",
                    "enum": ["auto", "cargo", "jest", "pytest", "go"],
                    "description": "Test framework to use (default: auto-detect)"
                }
            },
            "required": []
        })
    }

    fn is_write(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> Result<ToolOutput> {
        let scope = input["scope"].as_str().unwrap_or("all");
        let target = input["target"].as_str().unwrap_or_default();
        let framework_hint = input["framework"].as_str().unwrap_or("auto");

        let dir = self.resolve_dir();

        // 确定框架
        let framework = if framework_hint == "auto" {
            self.detect_framework(&dir).await
        } else {
            match framework_hint {
                "cargo" => TestFramework::Cargo,
                "jest" => TestFramework::Jest,
                "pytest" => TestFramework::Pytest,
                "go" => TestFramework::Go,
                _ => self.detect_framework(&dir).await,
            }
        };

        if framework == TestFramework::Unknown {
            return Ok(ToolOutput {
                content: "Error: could not detect test framework. No Cargo.toml, package.json, go.mod, or pytest.ini found.".to_string(),
                is_error: true,
            });
        }

        let (cmd_name, args) = match self.build_command(&framework, scope, target, &dir) {
            Some(c) => c,
            None => {
                return Ok(ToolOutput {
                    content: "Error: could not build test command".to_string(),
                    is_error: true,
                });
            }
        };

        let mut cmd = if let Some(user) = &self.run_as_user {
            let mut command = Command::new("sudo");
            let home = format!("HOME=/home/{user}");
            let path = format!(
                "PATH=/home/{user}/.cargo/bin:/home/{user}/.local/bin:/usr/local/bin:/usr/bin:/bin"
            );
            command.args([
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
                &cmd_name,
            ]);
            command
        } else {
            Command::new(&cmd_name)
        };
        cmd.args(&args);
        cmd.current_dir(&dir);

        let result = tokio::time::timeout(LONG_TOOL_TIMEOUT, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let code = output.status.code().unwrap_or(-1);

                let combined = format!(
                    "Framework: {:?}\nCommand: {} {}\nExit code: {}\n\n{}\n{}",
                    framework,
                    cmd_name,
                    args.join(" "),
                    code,
                    stdout,
                    stderr
                );

                // 截断过长输出
                let content = if combined.len() > 100_000 {
                    let half = 50_000;
                    format!(
                        "{}\n\n... [output truncated] ...\n\n{}",
                        &combined[..half],
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
                content: format!("Error running tests: {e}"),
                is_error: true,
            }),
            Err(_) => Ok(ToolOutput {
                content: format!(
                    "Error: test execution timed out after {}s",
                    LONG_TOOL_TIMEOUT.as_secs()
                ),
                is_error: true,
            }),
        }
    }
}

//! 飞书驱动的 monorepo 工作区与部署协调。
//!
//! Agent 只修改 Git worktree；真正安装和重启由 root-owned helper 在独立
//! systemd transient unit 中完成。两者之间只传递 UUID，部署内容来自本模块
//! 写入的结构化 manifest。

use crate::feishu::api::FeishuApi;
use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use hank_db::Deployment;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;

const RESULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RESULT_POLL_LIMIT: usize = 30 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployTarget {
    Core,
    Cli,
    Quant,
    QuantSlidev,
    Docs,
}

impl DeployTarget {
    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "server + admin",
            Self::Cli => "hank-cli",
            Self::Quant => "quant + quant web",
            Self::QuantSlidev => "quant Slidev",
            Self::Docs => "mdBook 文档",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DeploymentManifest {
    version: u8,
    #[serde(default)]
    action: DeploymentAction,
    deployment_id: String,
    source_dir: String,
    base_sha: String,
    commit_sha: String,
    targets: Vec<DeployTarget>,
    migration_required: bool,
    created_at: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DeploymentAction {
    #[default]
    Deploy,
    Rollback,
}

#[derive(Debug, Deserialize)]
struct DeploymentResultFile {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub struct PreparedDeployment {
    pub record: Deployment,
    pub targets: Vec<DeployTarget>,
    pub diff_stat: String,
    pub approval_label: &'static str,
}

/// 为 Trace/quant 飞书话题创建独立 worktree。配置关闭时调用方不应进入这里。
pub async fn prepare_repository_workspace(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<String> {
    let cfg = &state.config.server_agent;
    if !cfg.enabled {
        bail!("server_agent 未启用");
    }
    validate_uuid(session_id)?;

    let repo = canonical_dir(&cfg.repository_root)
        .with_context(|| format!("server_agent.repository_root 无效: {}", cfg.repository_root))?;
    if !repo.join(".git").exists() {
        bail!("{} 不是 Git 工作区", repo.display());
    }
    ensure_worktree_git_namespaces(&repo)
        .await
        .context("准备话题 Git 命名空间")?;

    let worktrees_root = PathBuf::from(&cfg.worktrees_root);
    tokio::fs::create_dir_all(&worktrees_root).await?;
    let worktree = worktrees_root.join(session_id);
    if worktree.join(".git").exists() {
        let worktree_str = worktree.to_string_lossy().into_owned();
        ensure_safe_directory_as_user(&worktree_str, &cfg.execution_user)
            .await
            .context("信任已有话题 worktree")?;
        return Ok(worktree_str);
    }
    if worktree.exists() {
        bail!(
            "worktree 路径已存在但不是 Git worktree: {}",
            worktree.display()
        );
    }

    let branch = format!("feishu/{}", session_id);
    let repo_str = repo.to_string_lossy().into_owned();
    ensure_safe_directory_as_user(&repo_str, &cfg.execution_user)
        .await
        .context("信任生产基线仓库")?;
    let output = git_command_as_user(&cfg.execution_user)
        .args(["-C", &repo_str])
        .args(["worktree", "add", "--no-checkout", "-b", &branch])
        .arg(&worktree)
        .arg(&cfg.base_ref)
        .output()
        .await
        .context("创建 Git worktree")?;
    if !output.status.success() {
        bail!("创建 Git worktree 失败: {}", command_error(&output));
    }
    let worktree_str = worktree.to_string_lossy().into_owned();
    let setup_result = async {
        ensure_safe_directory_as_user(&worktree_str, &cfg.execution_user)
            .await
            .context("信任新话题 worktree")?;
        git_as_user(&worktree_str, ["checkout", &branch], &cfg.execution_user)
            .await
            .context("checkout 话题分支")?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(e) = setup_result {
        cleanup_failed_session_workspace(&repo, &worktree, &branch, &cfg.execution_user).await;
        return Err(e).context("初始化话题 worktree");
    }
    Ok(worktree_str)
}

/// 为与 Trace/quant 无关的飞书话题创建普通隔离目录。
pub async fn prepare_general_workspace(state: &Arc<AppState>, session_id: &str) -> Result<String> {
    let cfg = &state.config.server_agent;
    if !cfg.enabled {
        bail!("server_agent 未启用");
    }
    validate_uuid(session_id)?;

    let root = canonical_dir(&cfg.general_workspaces_root).with_context(|| {
        format!(
            "server_agent.general_workspaces_root 无效: {}",
            cfg.general_workspaces_root
        )
    })?;
    let workspace = root.join(session_id);
    if workspace.is_dir() {
        return Ok(workspace.to_string_lossy().into_owned());
    }
    if workspace.exists() {
        bail!("普通工作区路径已存在但不是目录: {}", workspace.display());
    }

    let workspace_str = workspace.to_string_lossy().into_owned();
    let output = command_as_user(&cfg.execution_user, "install")
        .args(["-d", "-m", "2770", &workspace_str])
        .output()
        .await
        .context("创建普通隔离工作区")?;
    if !output.status.success() {
        bail!("创建普通隔离工作区失败: {}", command_error(&output));
    }
    Ok(workspace_str)
}

async fn cleanup_failed_session_workspace(
    repo: &Path,
    worktree: &Path,
    branch: &str,
    execution_user: &str,
) {
    let _ = git_command_as_user(execution_user)
        .args(["-C", &repo.to_string_lossy()])
        .args(["worktree", "remove", "--force"])
        .arg(worktree)
        .output()
        .await;
    // worktree remove 可能在 checkout 半途失败时只清掉 Git metadata；这个路径是
    // 刚为当前 session 创建的，确认仍为空/残留后才删除，避免遗留脏 worktree。
    if worktree.exists() {
        let _ = tokio::fs::remove_dir_all(worktree).await;
    }
    let branch_ref = format!("refs/heads/{branch}");
    let _ = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["update-ref", "-d", &branch_ref])
        .output()
        .await;
}

async fn ensure_worktree_git_namespaces(repo: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    for path in [
        repo.join(".git/worktrees"),
        repo.join(".git/refs/heads/feishu"),
        repo.join(".git/logs/refs/heads/feishu"),
    ] {
        tokio::fs::create_dir_all(&path)
            .await
            .with_context(|| format!("创建 {}", path.display()))?;
        let mut permissions = tokio::fs::metadata(&path).await?.permissions();
        permissions.set_mode(0o2770);
        tokio::fs::set_permissions(&path, permissions)
            .await
            .with_context(|| format!("设置 {} 权限", path.display()))?;
    }
    Ok(())
}

/// 把当前 worktree 固化为 commit，识别部署目标并创建待审批任务。
#[allow(clippy::too_many_arguments)]
pub async fn prepare_deployment(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
    account_id: &str,
    chat_id: &str,
    topic_id: &str,
) -> Result<PreparedDeployment> {
    ensure_server_agent_admin(state, user_id).await?;
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    ensure_repository_workspace(session.metadata.as_deref())?;
    let source_dir = session
        .work_dir
        .ok_or_else(|| anyhow!("当前会话没有 server worktree"))?;
    validate_worktree_root(state, &source_dir)?;

    let execution_user = state.config.server_agent.execution_user.as_str();
    let status = git_as_user(
        &source_dir,
        ["status", "--porcelain", "--untracked-files=all"],
        execution_user,
    )
    .await?;
    reject_forbidden_changes(status.lines().filter_map(status_path))?;
    if !status.trim().is_empty() {
        git_as_user(&source_dir, ["add", "-A"], execution_user).await?;
        let staged = git_as_user(
            &source_dir,
            ["diff", "--cached", "--name-only"],
            execution_user,
        )
        .await?;
        let staged_files: Vec<&str> = staged.lines().filter(|line| !line.is_empty()).collect();
        reject_forbidden_changes(staged_files.iter().copied())?;
        if !staged_files.is_empty() {
            let commit_message = format!("chore(agent): feishu session {}", &session_id[..8]);
            let mut command = git_command_as_user(execution_user);
            let output = command
                .args(["-C", &source_dir])
                .args([
                    "-c",
                    "user.name=Trace Agent",
                    "-c",
                    "user.email=agent@trace.local",
                ])
                .args(["commit", "-m", &commit_message])
                .output()
                .await?;
            if !output.status.success() {
                bail!("创建部署 commit 失败: {}", command_error(&output));
            }
        }
    }

    let commit_sha = git_as_user(&source_dir, ["rev-parse", "HEAD"], execution_user)
        .await?
        .trim()
        .to_string();
    let base_ref = state.config.server_agent.base_ref.as_str();
    let base_sha = git_as_user(&source_dir, ["rev-parse", base_ref], execution_user)
        .await?
        .trim()
        .to_string();
    let merge_base = git_as_user(
        &source_dir,
        ["merge-base", base_ref, "HEAD"],
        execution_user,
    )
    .await?
    .trim()
    .to_string();
    if merge_base != base_sha {
        bail!(
            "生产基线已包含其他话题的新部署；请先把当前分支 rebase 到 {base_ref}，或使用 /new 开启新话题"
        );
    }
    let range = format!("{base_ref}...HEAD");
    let changed = git_as_user(
        &source_dir,
        ["diff", "--name-only", range.as_str()],
        execution_user,
    )
    .await?;
    let changed_files: Vec<String> = changed
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if changed_files.is_empty() {
        bail!("当前工作区相对 {base_ref} 没有可部署变更");
    }
    reject_forbidden_changes(changed_files.iter().map(String::as_str))?;
    let (targets, migration_required, infra_changed) = classify_targets(&changed_files);
    if infra_changed {
        bail!("deploy/systemd/权限基础设施变更不能自部署，请通过 SSH 应急入口安装");
    }
    if migration_required {
        bail!("检测到 quant Alembic 迁移。请先在维护窗口人工执行并确认迁移，再部署应用代码");
    }
    if targets.is_empty() {
        bail!("这些变更没有匹配到可部署项目");
    }

    let diff_stat = git_as_user(
        &source_dir,
        ["diff", "--stat", range.as_str()],
        execution_user,
    )
    .await?;
    let target_json = serde_json::to_string(&targets)?;
    let labels = targets
        .iter()
        .map(|t| t.label())
        .collect::<Vec<_>>()
        .join("、");
    let summary = format!(
        "部署 {labels}；{} 个文件；commit {}",
        changed_files.len(),
        short_sha(&commit_sha)
    );
    let expires_at =
        Utc::now() + ChronoDuration::seconds(state.config.server_agent.approval_ttl_secs as i64);
    let record = state
        .db
        .create_deployment(
            session_id,
            user_id,
            account_id,
            chat_id,
            topic_id,
            &source_dir,
            &commit_sha,
            &target_json,
            &summary,
            expires_at,
        )
        .await?;

    write_manifest(
        state,
        &DeploymentManifest {
            version: 1,
            action: DeploymentAction::Deploy,
            deployment_id: record.id.clone(),
            source_dir,
            base_sha,
            commit_sha,
            targets: targets.clone(),
            migration_required,
            created_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    Ok(PreparedDeployment {
        record,
        targets,
        diff_stat,
        approval_label: "部署",
    })
}

/// 对最近一次成功发布创建回滚审批。回滚只切 release 和生产 Git 基线，
/// 不运行构建、测试或数据库迁移。
#[allow(clippy::too_many_arguments)]
pub async fn prepare_rollback(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
    account_id: &str,
    chat_id: &str,
    topic_id: &str,
) -> Result<PreparedDeployment> {
    ensure_server_agent_admin(state, user_id).await?;
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    ensure_repository_workspace(session.metadata.as_deref())?;
    let latest = state
        .db
        .get_latest_successful_deployment()
        .await?
        .ok_or_else(|| anyhow!("没有可回滚的成功发布"))?;
    let manifest = read_manifest(state, &latest.id).await?;
    if manifest.action != DeploymentAction::Deploy {
        bail!("最近一次成功操作已经是回滚，请先完成新的部署");
    }

    let current_sha = git(
        &state.config.server_agent.repository_root,
        ["rev-parse", state.config.server_agent.base_ref.as_str()],
    )
    .await?
    .trim()
    .to_string();
    if current_sha != latest.commit_sha || current_sha != manifest.commit_sha {
        bail!("生产 Git 基线与最近成功发布不一致，请使用 SSH 应急入口核对");
    }

    let targets: Vec<DeployTarget> = serde_json::from_str(&latest.targets)?;
    if targets.is_empty() {
        bail!("最近成功发布没有可回滚目标");
    }
    let labels = targets
        .iter()
        .map(|target| target.label())
        .collect::<Vec<_>>()
        .join("、");
    let summary = format!(
        "回滚 {labels}；生产基线 {} -> {}",
        short_sha(&manifest.commit_sha),
        short_sha(&manifest.base_sha)
    );
    let expires_at =
        Utc::now() + ChronoDuration::seconds(state.config.server_agent.approval_ttl_secs as i64);
    let target_json = serde_json::to_string(&targets)?;
    let record = state
        .db
        .create_deployment(
            session_id,
            user_id,
            account_id,
            chat_id,
            topic_id,
            &latest.source_dir,
            &current_sha,
            &target_json,
            &summary,
            expires_at,
        )
        .await?;
    write_manifest(
        state,
        &DeploymentManifest {
            version: 1,
            action: DeploymentAction::Rollback,
            deployment_id: record.id.clone(),
            source_dir: latest.source_dir,
            base_sha: manifest.base_sha,
            commit_sha: current_sha,
            targets: targets.clone(),
            migration_required: false,
            created_at: Utc::now().to_rfc3339(),
        },
    )
    .await?;

    Ok(PreparedDeployment {
        record,
        targets,
        diff_stat: "将各目标 current 切换到 previous；失败时自动恢复当前版本".to_string(),
        approval_label: "回滚",
    })
}

pub async fn workspace_diff(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
) -> Result<String> {
    ensure_server_agent_admin(state, user_id).await?;
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    ensure_repository_workspace(session.metadata.as_deref())?;
    let worktree = session
        .work_dir
        .ok_or_else(|| anyhow!("当前会话没有 server worktree"))?;
    validate_worktree_root(state, &worktree)?;
    let execution_user = state.config.server_agent.execution_user.as_str();
    let status = git_as_user(&worktree, ["status", "--short"], execution_user).await?;
    reject_forbidden_changes(status.lines().filter_map(status_path))?;
    let base_ref = state.config.server_agent.base_ref.as_str();
    let range = format!("{base_ref}...HEAD");
    let committed = git_as_user(
        &worktree,
        ["diff", "--stat", range.as_str()],
        execution_user,
    )
    .await?;
    let committed_patch = git_as_user(
        &worktree,
        ["diff", "--no-color", "--unified=3", range.as_str()],
        execution_user,
    )
    .await?;
    let unstaged = git_as_user(&worktree, ["diff", "--stat"], execution_user).await?;
    let unstaged_patch = git_as_user(
        &worktree,
        ["diff", "--no-color", "--unified=3"],
        execution_user,
    )
    .await?;
    let staged_patch = git_as_user(
        &worktree,
        ["diff", "--cached", "--no-color", "--unified=3"],
        execution_user,
    )
    .await?;
    let mut parts = Vec::new();
    if !status.trim().is_empty() {
        parts.push(format!("工作区状态：\n{}", status.trim()));
    }
    if !committed.trim().is_empty() {
        parts.push(format!("已提交变更：\n{}", committed.trim()));
    }
    if !unstaged.trim().is_empty() {
        parts.push(format!("未提交变更：\n{}", unstaged.trim()));
    }
    let patch = [committed_patch, staged_patch, unstaged_patch]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !patch.is_empty() {
        let truncated: String = patch.chars().take(7000).collect();
        let suffix = if patch.chars().count() > 7000 {
            "\n...（diff 已截断，可拆分需求后再审阅）"
        } else {
            ""
        };
        parts.push(format!("代码 diff：\n```diff\n{truncated}\n```{suffix}"));
    }
    if parts.is_empty() {
        Ok("当前话题工作区没有变更".to_string())
    } else {
        Ok(parts.join("\n\n"))
    }
}

/// 按受影响目标运行固定测试矩阵。命令和参数由服务端决定，不接收飞书里的
/// 任意 shell 文本；测试只在当前话题 worktree 中执行。
pub async fn test_workspace(
    state: &Arc<AppState>,
    session_id: &str,
    user_id: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String> {
    ensure_server_agent_admin(state, user_id).await?;
    let session = state
        .db
        .get_session(session_id)
        .await?
        .ok_or_else(|| anyhow!("会话不存在"))?;
    ensure_repository_workspace(session.metadata.as_deref())?;
    let worktree = session
        .work_dir
        .ok_or_else(|| anyhow!("当前会话没有 server worktree"))?;
    validate_worktree_root(state, &worktree)?;

    let base_ref = state.config.server_agent.base_ref.as_str();
    let range = format!("{base_ref}...HEAD");
    let execution_user = state.config.server_agent.execution_user.as_str();
    let committed = git_as_user(
        &worktree,
        ["diff", "--name-only", range.as_str()],
        execution_user,
    )
    .await?;
    let status = git_as_user(
        &worktree,
        ["status", "--porcelain", "--untracked-files=all"],
        execution_user,
    )
    .await?;
    let mut changed_files = committed
        .lines()
        .chain(status.lines().filter_map(status_path))
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    changed_files.sort();
    changed_files.dedup();
    if changed_files.is_empty() {
        bail!("当前工作区没有需要测试的变更");
    }
    reject_forbidden_changes(changed_files.iter().map(String::as_str))?;
    let (targets, migration_required, infra_changed) = classify_targets(&changed_files);
    if infra_changed {
        bail!("存在不能自部署的基础设施或未知路径变更");
    }
    if migration_required {
        bail!("检测到 quant Alembic 迁移；需先进入维护窗口，常规 /test 不执行迁移");
    }

    let execution_user = execution_user.to_string();
    let mut passed = Vec::new();
    for target in targets {
        match target {
            DeployTarget::Core => {
                run_checked(
                    &worktree,
                    "cargo test --workspace",
                    "cargo",
                    &["test", "--workspace", "--locked"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &format!("{worktree}/admin"),
                    "admin pnpm install",
                    "pnpm",
                    &["install", "--frozen-lockfile"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &format!("{worktree}/admin"),
                    "admin pnpm build",
                    "pnpm",
                    &["build"],
                    cancel,
                    &execution_user,
                )
                .await?;
            }
            DeployTarget::Cli => {
                run_checked(
                    &format!("{worktree}/cli"),
                    "hank-cli cargo test",
                    "cargo",
                    &["test", "--locked"],
                    cancel,
                    &execution_user,
                )
                .await?;
            }
            DeployTarget::Quant => {
                let quant = format!("{worktree}/quant");
                let quant_web = format!("{quant}/web");
                run_checked(
                    &quant_web,
                    "quant web pnpm install",
                    "pnpm",
                    &["install", "--frozen-lockfile"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &quant_web,
                    "quant web test",
                    "pnpm",
                    &["test"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &quant_web,
                    "quant web build",
                    "pnpm",
                    &["build"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &quant,
                    "quant uv sync",
                    "uv",
                    &["sync", "--frozen"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &quant,
                    "quant pytest",
                    "uv",
                    &["run", "pytest", "tests/"],
                    cancel,
                    &execution_user,
                )
                .await?;
            }
            DeployTarget::QuantSlidev => {
                let slidev = format!("{worktree}/quant/slidev");
                run_checked(
                    &slidev,
                    "Slidev pnpm install",
                    "pnpm",
                    &["install", "--frozen-lockfile"],
                    cancel,
                    &execution_user,
                )
                .await?;
                run_checked(
                    &slidev,
                    "Slidev build",
                    "pnpm",
                    &["build"],
                    cancel,
                    &execution_user,
                )
                .await?;
            }
            DeployTarget::Docs => {
                let destination = format!("/tmp/hank-docs-test-{session_id}");
                let result = run_checked(
                    &worktree,
                    "mdBook build",
                    "mdbook",
                    &["build", "docs", "--dest-dir", &destination],
                    cancel,
                    &execution_user,
                )
                .await;
                let _ = tokio::fs::remove_dir_all(&destination).await;
                result?;
            }
        }
        passed.push(target.label());
    }
    Ok(format!("已通过：{}", passed.join("、")))
}

async fn run_checked(
    cwd: &str,
    label: &str,
    program: &str,
    args: &[&str],
    cancel: &tokio_util::sync::CancellationToken,
    execution_user: &str,
) -> Result<()> {
    let mut command = Command::new("sudo");
    let home = format!("HOME=/home/{execution_user}");
    let path = format!(
        "PATH=/home/{execution_user}/.cargo/bin:/home/{execution_user}/.local/bin:/usr/local/bin:/usr/bin:/bin"
    );
    command
        .args([
            "--non-interactive",
            "--user",
            execution_user,
            "--",
            "env",
            &home,
            &path,
            program,
        ])
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true);
    let output = tokio::select! {
        _ = cancel.cancelled() => bail!("测试已取消"),
        result = command.output() => result.with_context(|| format!("启动 {label}"))?,
    };
    if output.status.success() {
        return Ok(());
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    bail!("{label} 失败：\n{}", tail_chars(&combined, 6000));
}

fn tail_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    value
        .chars()
        .skip(count.saturating_sub(max_chars))
        .collect()
}

pub async fn start_deployment(state: Arc<AppState>, deployment: Deployment) {
    let id = deployment.id.clone();
    if let Err(e) = state
        .db
        .update_deployment_status(&id, "starting", None, None)
        .await
    {
        tracing::error!(deployment_id = %id, "更新部署状态失败: {e:#}");
        return;
    }

    let cfg = state.config.server_agent.clone();
    let mut command = if cfg.deploy_use_sudo {
        let mut cmd = Command::new("sudo");
        cmd.arg("--").arg(&cfg.deploy_helper);
        cmd
    } else {
        Command::new(&cfg.deploy_helper)
    };
    let launch = command.arg(&id).output().await;
    match launch {
        Ok(output) if output.status.success() => {
            tracing::info!(deployment_id = %id, "独立部署任务已启动");
            spawn_result_monitor(state, deployment);
        }
        Ok(output) => {
            let error = format!("启动部署 helper 失败: {}", command_error(&output));
            let _ = state
                .db
                .update_deployment_status(&id, "failed", None, Some(&error))
                .await;
            notify_terminal(&state, &deployment, "failed", None, Some(&error)).await;
        }
        Err(e) => {
            let error = format!("无法执行部署 helper: {e}");
            let _ = state
                .db
                .update_deployment_status(&id, "failed", None, Some(&error))
                .await;
            notify_terminal(&state, &deployment, "failed", None, Some(&error)).await;
        }
    }
}

/// server 启动后恢复对独立部署进程的监听。
pub fn recover_deployments(state: Arc<AppState>) {
    if !state.config.server_agent.enabled {
        return;
    }
    tokio::spawn(async move {
        match state.db.list_recoverable_deployments().await {
            Ok(records) => {
                for record in records {
                    spawn_result_monitor(state.clone(), record);
                }
            }
            Err(e) => tracing::warn!("恢复部署任务失败: {e:#}"),
        }
    });
}

fn spawn_result_monitor(state: Arc<AppState>, deployment: Deployment) {
    tokio::spawn(async move {
        if let Err(e) = monitor_result(&state, &deployment).await {
            tracing::warn!(deployment_id = %deployment.id, "部署结果监听失败: {e:#}");
        }
    });
}

async fn monitor_result(state: &Arc<AppState>, deployment: &Deployment) -> Result<()> {
    let result_path = job_path(state, &deployment.id, "result.json")?;
    let mut last_status = String::new();
    for _ in 0..RESULT_POLL_LIMIT {
        if let Ok(content) = tokio::fs::read_to_string(&result_path).await {
            let result: DeploymentResultFile = serde_json::from_str(&content)
                .with_context(|| format!("解析 {}", result_path.display()))?;
            if result.status != last_status {
                let terminal = matches!(
                    result.status.as_str(),
                    "succeeded" | "failed" | "rolled_back"
                );
                state
                    .db
                    .update_deployment_status(
                        &deployment.id,
                        &result.status,
                        result.message.as_deref(),
                        result.error.as_deref(),
                    )
                    .await?;
                last_status = result.status.clone();
                if terminal {
                    notify_terminal(
                        state,
                        deployment,
                        &result.status,
                        result.message.as_deref(),
                        result.error.as_deref(),
                    )
                    .await;
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(RESULT_POLL_INTERVAL).await;
    }
    let error = "部署任务超过 60 分钟仍无终态，请检查 systemd 部署单元";
    state
        .db
        .update_deployment_status(&deployment.id, "failed", None, Some(error))
        .await?;
    notify_terminal(state, deployment, "failed", None, Some(error)).await;
    Ok(())
}

async fn notify_terminal(
    state: &Arc<AppState>,
    deployment: &Deployment,
    status: &str,
    message: Option<&str>,
    error: Option<&str>,
) {
    let Ok(Some(account)) = state.db.get_feishu_account(&deployment.account_id).await else {
        return;
    };
    let api = FeishuApi::new_archived(&account, state.db.clone());
    let operation = if deployment.summary.starts_with("回滚 ") {
        "回滚"
    } else {
        "部署"
    };
    let text = match status {
        "succeeded" => format!(
            "{operation}成功\n{}\n{}",
            deployment.summary,
            message.unwrap_or("健康检查通过")
        ),
        "rolled_back" => format!(
            "{operation}失败，已自动恢复\n{}\n{}",
            deployment.summary,
            error.or(message).unwrap_or("未知错误")
        ),
        _ => format!(
            "{operation}失败\n{}\n{}",
            deployment.summary,
            error.or(message).unwrap_or("未知错误")
        ),
    };
    if let Some(card_id) = &deployment.card_message_id {
        let _ = api
            .reply_text(card_id, &text, deployment.topic_id != "main")
            .await;
    } else {
        let _ = api.send_text("chat_id", &deployment.chat_id, &text).await;
    }
}

pub async fn ensure_server_agent_admin(state: &Arc<AppState>, user_id: &str) -> Result<()> {
    let user = state
        .db
        .get_user_by_id(user_id)
        .await?
        .ok_or_else(|| anyhow!("Trace 用户不存在"))?;
    if !user.can_login_admin {
        bail!("只有 Trace 管理员可以使用 server-only 代码工作区");
    }
    Ok(())
}

fn classify_targets(paths: &[String]) -> (Vec<DeployTarget>, bool, bool) {
    let mut targets = Vec::new();
    let mut migration_required = false;
    let mut infra_changed = false;
    for path in paths {
        let target = if path.starts_with("server/")
            || path.starts_with("crates/")
            || path.starts_with("admin/")
            || matches!(path.as_str(), "Cargo.toml" | "Cargo.lock")
        {
            Some(DeployTarget::Core)
        } else if path.starts_with("cli/") {
            Some(DeployTarget::Cli)
        } else if path.starts_with("quant/slidev/") {
            Some(DeployTarget::QuantSlidev)
        } else if path.starts_with("quant/") {
            if path.starts_with("quant/alembic/") {
                migration_required = true;
            }
            Some(DeployTarget::Quant)
        } else if path.starts_with("docs/") || matches!(path.as_str(), "README.md" | "AGENTS.md") {
            Some(DeployTarget::Docs)
        } else if path.starts_with("deploy/") || path == "Makefile" || path == "config.example.toml"
        {
            infra_changed = true;
            None
        } else {
            infra_changed = true;
            None
        };
        if let Some(target) = target {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    (targets, migration_required, infra_changed)
}

fn reject_forbidden_changes<'a>(paths: impl Iterator<Item = &'a str>) -> Result<()> {
    for path in paths {
        let path = path.trim_matches('"');
        if path == "client" || path.starts_with("client/") || path.contains(" -> client/") {
            bail!("client/ 已从飞书迭代范围排除，请撤销该目录的修改");
        }
        if path == "config.toml" || path.ends_with("/config.toml") {
            bail!("生产配置文件不允许进入 Agent 部署提交: {path}");
        }
    }
    Ok(())
}

fn status_path(line: &str) -> Option<&str> {
    line.get(3..).map(str::trim).filter(|path| !path.is_empty())
}

async fn git<const N: usize>(worktree: &str, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        bail!("git 命令失败: {}", command_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn command_as_user(user: &str, program: &str) -> Command {
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
        program,
    ]);
    command
}

fn git_command_as_user(user: &str) -> Command {
    command_as_user(user, "git")
}

/// 新会话显式记录 workspace_kind；旧 server_agent 会话没有该字段，兼容为 repository。
pub fn is_repository_workspace_metadata(metadata: Option<&str>) -> bool {
    let Some(value) = metadata.and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return false;
    };
    match value["workspace_kind"].as_str() {
        Some("repository") => true,
        Some(_) => false,
        None => value["server_agent"].as_bool().unwrap_or(false),
    }
}

fn ensure_repository_workspace(metadata: Option<&str>) -> Result<()> {
    if !is_repository_workspace_metadata(metadata) {
        bail!("当前话题是普通隔离工作区，不支持代码 diff、测试、部署或回滚");
    }
    Ok(())
}

async fn ensure_safe_directory_as_user(worktree: &str, user: &str) -> Result<()> {
    let current = git_command_as_user(user)
        .args(["config", "--global", "--get-all", "safe.directory"])
        .output()
        .await
        .context("读取 Git safe.directory")?;
    if !current.status.success() && current.status.code() != Some(1) {
        bail!("读取 Git safe.directory 失败: {}", command_error(&current));
    }
    if String::from_utf8_lossy(&current.stdout)
        .lines()
        .any(|configured| configured == worktree)
    {
        return Ok(());
    }

    let output = git_command_as_user(user)
        .args(["config", "--global", "--add", "safe.directory", worktree])
        .output()
        .await
        .context("写入 Git safe.directory")?;
    if !output.status.success() {
        bail!("写入 Git safe.directory 失败: {}", command_error(&output));
    }
    Ok(())
}

async fn git_as_user<const N: usize>(
    worktree: &str,
    args: [&str; N],
    user: &str,
) -> Result<String> {
    let output = git_command_as_user(user)
        .arg("-C")
        .arg(worktree)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        bail!("git 命令失败: {}", command_error(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn write_manifest(state: &Arc<AppState>, manifest: &DeploymentManifest) -> Result<()> {
    let dir = PathBuf::from(&state.config.server_agent.deploy_jobs_dir);
    tokio::fs::create_dir_all(&dir).await?;
    let path = job_path(state, &manifest.deployment_id, "json")?;
    let temp = job_path(state, &manifest.deployment_id, "json.tmp")?;
    let content = serde_json::to_vec_pretty(manifest)?;
    tokio::fs::write(&temp, content).await?;
    tokio::fs::rename(temp, path).await?;
    Ok(())
}

async fn read_manifest(state: &Arc<AppState>, id: &str) -> Result<DeploymentManifest> {
    let path = job_path(state, id, "json")?;
    let content = tokio::fs::read(&path)
        .await
        .with_context(|| format!("读取历史发布 manifest: {}", path.display()))?;
    serde_json::from_slice(&content).context("解析历史发布 manifest")
}

fn job_path(state: &AppState, id: &str, suffix: &str) -> Result<PathBuf> {
    validate_uuid(id)?;
    Ok(Path::new(&state.config.server_agent.deploy_jobs_dir).join(format!("{id}.{suffix}")))
}

fn validate_worktree_root(state: &AppState, path: &str) -> Result<()> {
    let root = canonical_dir(&state.config.server_agent.worktrees_root)?;
    let candidate = canonical_dir(path)?;
    if !candidate.starts_with(&root) {
        bail!("会话工作区不在 server_agent.worktrees_root 内");
    }
    Ok(())
}

fn canonical_dir(path: &str) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_dir() {
        bail!("路径不是目录: {}", canonical.display());
    }
    Ok(canonical)
}

fn validate_uuid(value: &str) -> Result<()> {
    uuid::Uuid::parse_str(value).map_err(|_| anyhow!("非法任务 ID"))?;
    Ok(())
}

fn command_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    text.trim().chars().take(2000).collect()
}

fn short_sha(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_all_monorepo_targets() {
        let paths = vec![
            "server/src/main.rs".to_string(),
            "admin/src/App.vue".to_string(),
            "cli/src/main.rs".to_string(),
            "quant/app/main.py".to_string(),
            "quant/slidev/slides.md".to_string(),
            "docs/src/overview.md".to_string(),
        ];
        let (targets, migration, infra) = classify_targets(&paths);
        assert_eq!(
            targets,
            vec![
                DeployTarget::Core,
                DeployTarget::Cli,
                DeployTarget::Quant,
                DeployTarget::QuantSlidev,
                DeployTarget::Docs,
            ]
        );
        assert!(!migration);
        assert!(!infra);
    }

    #[test]
    fn detects_migrations_and_infrastructure() {
        let paths = vec![
            "quant/alembic/versions/0023_x.py".to_string(),
            "deploy/hank-server.service".to_string(),
        ];
        let (_, migration, infra) = classify_targets(&paths);
        assert!(migration);
        assert!(infra);
    }

    #[test]
    fn rejects_client_and_config() {
        assert!(reject_forbidden_changes(["client/src/App.vue"].into_iter()).is_err());
        assert!(reject_forbidden_changes(["quant/config.toml"].into_iter()).is_err());
    }

    #[test]
    fn recognizes_new_and_legacy_repository_metadata() {
        assert!(is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"workspace_kind":"repository"}"#
        )));
        assert!(is_repository_workspace_metadata(Some(
            r#"{"server_agent":true}"#
        )));
        assert!(!is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"workspace_kind":"general"}"#
        )));
        assert!(!is_repository_workspace_metadata(Some(
            r#"{"server_agent":true,"agent_kind":"conversation","workspace_kind":"none"}"#
        )));
        assert!(!is_repository_workspace_metadata(None));
    }

    #[test]
    fn old_manifest_without_action_defaults_to_deploy() {
        let manifest: DeploymentManifest = serde_json::from_value(serde_json::json!({
            "version": 1,
            "deployment_id": "9f2f881f-7668-4ed7-9e7f-181f2550af54",
            "source_dir": "/opt/hank-worktrees/9f2f881f-7668-4ed7-9e7f-181f2550af54",
            "base_sha": "1111111111111111111111111111111111111111",
            "commit_sha": "2222222222222222222222222222222222222222",
            "targets": ["core"],
            "migration_required": false,
            "created_at": "2026-07-31T00:00:00Z"
        }))
        .expect("旧 manifest 应保持兼容");

        assert_eq!(manifest.action, DeploymentAction::Deploy);
    }
}

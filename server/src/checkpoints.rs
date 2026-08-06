use crate::AppState;
use axum::{
    extract::{Path, State},
    response::IntoResponse,
    Extension,
};
use std::sync::Arc;
use tokio::process::Command;
use tracing;

use crate::auth::Claims;
use crate::response as R;

// ─── Git Checkpoint Operations ───────────────────────────────────────────────

/// 检查 work_dir 是否是 git 仓库
async fn is_git_repo(work_dir: &str) -> bool {
    Command::new("git")
        .args(["-C", work_dir, "rev-parse", "--git-dir"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 在 orphan 分支上创建 checkpoint commit（不切换分支、不影响工作区）
async fn git_create_checkpoint(
    work_dir: &str,
    branch: &str,
    label: &str,
) -> anyhow::Result<String> {
    // 1. 将当前工作区所有文件加入 index（临时）
    let add_output = Command::new("git")
        .args(["-C", work_dir, "add", "-A"])
        .output()
        .await?;
    if !add_output.status.success() {
        anyhow::bail!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add_output.stderr)
        );
    }

    // 2. 写入 tree object
    let tree_output = Command::new("git")
        .args(["-C", work_dir, "write-tree"])
        .output()
        .await?;
    if !tree_output.status.success() {
        // 恢复 index
        let _ = Command::new("git")
            .args(["-C", work_dir, "reset"])
            .output()
            .await;
        anyhow::bail!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&tree_output.stderr)
        );
    }
    let tree_sha = String::from_utf8_lossy(&tree_output.stdout)
        .trim()
        .to_string();

    // 3. 恢复 index（撤销 add -A）
    let _ = Command::new("git")
        .args(["-C", work_dir, "reset"])
        .output()
        .await;

    // 4. 检查 orphan 分支是否存在
    let ref_name = format!("refs/heads/{}", branch);
    let parent_output = Command::new("git")
        .args(["-C", work_dir, "rev-parse", "--verify", &ref_name])
        .output()
        .await?;

    // 5. 创建 commit（有 parent 或无 parent）
    let commit_msg = format!("checkpoint: {}", label);
    let commit_sha = if parent_output.status.success() {
        let parent_sha = String::from_utf8_lossy(&parent_output.stdout)
            .trim()
            .to_string();
        let output = Command::new("git")
            .args([
                "-C",
                work_dir,
                "commit-tree",
                &tree_sha,
                "-p",
                &parent_sha,
                "-m",
                &commit_msg,
            ])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!(
                "git commit-tree failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        let output = Command::new("git")
            .args(["-C", work_dir, "commit-tree", &tree_sha, "-m", &commit_msg])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!(
                "git commit-tree failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    // 6. 更新 ref 指向新 commit
    let update_output = Command::new("git")
        .args(["-C", work_dir, "update-ref", &ref_name, &commit_sha])
        .output()
        .await?;
    if !update_output.status.success() {
        anyhow::bail!(
            "git update-ref failed: {}",
            String::from_utf8_lossy(&update_output.stderr)
        );
    }

    Ok(commit_sha)
}

/// 恢复工作区到指定 commit 的状态
async fn git_restore_checkpoint(work_dir: &str, commit_sha: &str) -> anyhow::Result<()> {
    // 使用 git checkout <sha> -- . 恢复所有文件
    let output = Command::new("git")
        .args(["-C", work_dir, "checkout", commit_sha, "--", "."])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!(
            "git checkout failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // reset index 回到干净状态
    let _ = Command::new("git")
        .args(["-C", work_dir, "reset"])
        .output()
        .await;
    Ok(())
}

// ─── Public: Checkpoint 创建入口 ─────────────────────────────────────────────

/// 在 agent turn 开始前创建 checkpoint（由 chat.rs 调用）
pub async fn create_checkpoint_for_turn(
    state: &AppState,
    session_id: &str,
    message_id: &str,
    work_dir: &str,
    label: &str,
) -> anyhow::Result<()> {
    if !is_git_repo(work_dir).await {
        tracing::debug!(
            session_id,
            "work_dir is not a git repo, skipping checkpoint"
        );
        return Ok(());
    }

    let branch = format!("hank/checkpoints/{}", session_id);
    let truncated_label: String = label.chars().take(40).collect();

    // 创建 git checkpoint
    let commit_sha = git_create_checkpoint(work_dir, &branch, &truncated_label).await?;

    // 捕获 spec 快照
    let spec_snapshot = capture_spec_snapshot(&state.db).await?;

    // 存入数据库
    state
        .db
        .create_checkpoint(
            session_id,
            message_id,
            &commit_sha,
            &branch,
            spec_snapshot.as_deref(),
            &truncated_label,
        )
        .await?;

    tracing::info!(session_id, commit_sha = %commit_sha, "checkpoint created");
    Ok(())
}

/// 捕获当前所有 spec 的快照
async fn capture_spec_snapshot(db: &hank_db::Database) -> anyhow::Result<Option<String>> {
    let specs = db.list_specs().await?;
    if specs.is_empty() {
        return Ok(None);
    }
    let snapshot: Vec<serde_json::Value> = specs
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "capability": s.capability,
                "title": s.title,
                "content": s.content,
                "metadata": s.metadata,
                "version": s.version,
            })
        })
        .collect();
    Ok(Some(serde_json::to_string(&snapshot)?))
}

/// 从快照恢复 spec 状态
async fn restore_spec_snapshot(db: &hank_db::Database, snapshot_json: &str) -> anyhow::Result<()> {
    let snapshot: Vec<serde_json::Value> = serde_json::from_str(snapshot_json)?;

    for item in &snapshot {
        let id = item["id"].as_str().unwrap_or_default();
        let content = item["content"].as_str().unwrap_or_default();
        let metadata = item["metadata"].as_str();
        let title = item["title"].as_str();

        // 尝试更新，如果 spec 不存在则创建
        if db.get_spec(id).await?.is_some() {
            db.update_spec(id, Some(content), metadata, title).await?;
        } else {
            let capability = item["capability"].as_str().unwrap_or_default();
            let title = item["title"].as_str().unwrap_or(capability);
            db.create_spec(capability, title, content, metadata).await?;
        }
    }
    Ok(())
}

// ─── API Handlers ────────────────────────────────────────────────────────────

/// GET /api/sessions/{id}/checkpoints
pub async fn list_checkpoints_handler(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.db.list_checkpoints(&session_id).await {
        Ok(checkpoints) => R::ok(&checkpoints),
        Err(e) => R::internal_error(e),
    }
}

/// POST /api/sessions/{id}/rewind/{checkpoint_id}
pub async fn rewind_handler(
    State(state): State<Arc<AppState>>,
    Extension(_claims): Extension<Claims>,
    Path((session_id, checkpoint_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // 1. 检查是否有正在运行的 agent task
    {
        let tasks = state.active_tasks.read().await;
        if tasks.contains_key(&session_id) {
            return R::conflict("agent task is running, cannot rewind");
        }
    }

    // 2. 获取 checkpoint
    let checkpoint = match state.db.get_checkpoint(&checkpoint_id).await {
        Ok(Some(cp)) => cp,
        Ok(None) => return R::not_found("checkpoint not found"),
        Err(e) => return R::internal_error(e),
    };
    if checkpoint.session_id != session_id {
        return R::not_found("checkpoint not found");
    }

    // 3. 获取 session 的 work_dir
    let session = match state.db.get_session(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return R::not_found("session not found"),
        Err(e) => return R::internal_error(e),
    };

    // 4. 恢复 git 文件状态
    if let Some(ref work_dir) = session.work_dir {
        if let Err(e) = git_restore_checkpoint(work_dir, &checkpoint.git_commit_sha).await {
            return R::internal_error(e);
        }
    }

    // 5. 恢复 spec 快照
    if let Some(ref snapshot) = checkpoint.spec_snapshot {
        if let Err(e) = restore_spec_snapshot(&state.db, &snapshot.to_string()).await {
            tracing::warn!(session_id = %session_id, "Failed to restore spec snapshot: {e:#}");
        }
    }

    // 6. 更新 session 的 active_leaf_id 到 checkpoint 的 message_id
    if let Err(e) = state
        .db
        .update_active_leaf(&session_id, &checkpoint.message_id)
        .await
    {
        return R::internal_error(e);
    }

    // 7. 删除该 checkpoint 之后的 checkpoints
    if let Err(e) = state
        .db
        .delete_checkpoints_after(&session_id, checkpoint.created_at)
        .await
    {
        tracing::warn!(session_id = %session_id, "Failed to cleanup later checkpoints: {e:#}");
    }

    tracing::info!(session_id = %session_id, checkpoint_id = %checkpoint_id, "rewound to checkpoint");
    R::no_content()
}

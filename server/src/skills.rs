use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct SkillQuery {
    pub work_dir: String,
}

#[derive(Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub source: String,
    pub installed: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SkillLockEntry {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub skill_path: String,
    #[serde(default)]
    pub hash: String,
}

// GET /api/skills?work_dir=xxx
pub async fn list_skills(
    State(_state): State<Arc<AppState>>,
    Query(query): Query<SkillQuery>,
) -> impl IntoResponse {
    let work_dir = PathBuf::from(&query.work_dir);
    let skills_dir = work_dir.join(".agents").join("skills");
    let lock_path = work_dir.join("skills-lock.json");

    // 读取 lock file
    let lock_entries: Vec<SkillLockEntry> = match tokio::fs::read_to_string(&lock_path).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    let mut skills: Vec<SkillInfo> = Vec::new();

    // 遍历 skills 目录
    if skills_dir.exists() {
        let mut entries = match tokio::fs::read_dir(&skills_dir).await {
            Ok(e) => e,
            Err(e) => return R::internal_error(format!("读取 skills 目录失败: {}", e)),
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let skill_md = path.join("SKILL.md");

            let (description, _) = parse_skill_frontmatter(&skill_md).await;
            let source = lock_entries
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.source.clone())
                .unwrap_or_default();

            skills.push(SkillInfo {
                name,
                description,
                source,
                installed: true,
            });
        }
    }

    R::ok(serde_json::json!(skills))
}

#[derive(Deserialize)]
pub struct InstallSkillRequest {
    pub source: String,     // "owner/repo"
    pub skill_name: String, // skill 名称
    pub work_dir: String,
    pub skill_path: Option<String>, // 仓库内路径，默认 "skill/SKILL.md"
}

// POST /api/skills/install
pub async fn install_skill(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<InstallSkillRequest>,
) -> impl IntoResponse {
    let work_dir = PathBuf::from(&body.work_dir);
    let skills_dir = work_dir
        .join(".agents")
        .join("skills")
        .join(&body.skill_name);
    let lock_path = work_dir.join("skills-lock.json");

    // 确定下载路径
    let skill_path = body
        .skill_path
        .clone()
        .unwrap_or_else(|| "skill/SKILL.md".to_string());

    // 从 GitHub raw 下载
    let url = format!(
        "https://raw.githubusercontent.com/{}/main/{}",
        body.source, skill_path
    );

    let client = reqwest::Client::new();
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return R::internal_error(format!("GitHub 请求失败: {}", e)),
    };

    if !resp.status().is_success() {
        // 尝试 master 分支
        let url_master = format!(
            "https://raw.githubusercontent.com/{}/master/{}",
            body.source, skill_path
        );
        let resp2 = match client.get(&url_master).send().await {
            Ok(r) => r,
            Err(e) => return R::internal_error(format!("GitHub 请求失败: {}", e)),
        };
        if !resp2.status().is_success() {
            return R::bad_request(format!("无法从 GitHub 下载 skill: HTTP {}", resp2.status()));
        }
        let content = match resp2.text().await {
            Ok(c) => c,
            Err(e) => return R::internal_error(format!("读取响应失败: {}", e)),
        };
        return save_skill(&skills_dir, &lock_path, &body, &content, &skill_path).await;
    }

    let content = match resp.text().await {
        Ok(c) => c,
        Err(e) => return R::internal_error(format!("读取响应失败: {}", e)),
    };

    save_skill(&skills_dir, &lock_path, &body, &content, &skill_path).await
}

async fn save_skill(
    skills_dir: &PathBuf,
    lock_path: &PathBuf,
    body: &InstallSkillRequest,
    content: &str,
    skill_path: &str,
) -> axum::response::Response {
    // 创建目录
    if let Err(e) = tokio::fs::create_dir_all(skills_dir).await {
        return R::internal_error(format!("创建目录失败: {}", e));
    }

    // 写入 SKILL.md
    let skill_file = skills_dir.join("SKILL.md");
    if let Err(e) = tokio::fs::write(&skill_file, content).await {
        return R::internal_error(format!("写入文件失败: {}", e));
    }

    // 更新 lock file
    let mut lock_entries: Vec<SkillLockEntry> = match tokio::fs::read_to_string(lock_path).await {
        Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    // 移除已有同名条目
    lock_entries.retain(|e| e.name != body.skill_name);

    // 计算简单 hash
    let hash = format!("{:x}", md5_hash(content));

    lock_entries.push(SkillLockEntry {
        name: body.skill_name.clone(),
        source: body.source.clone(),
        skill_path: skill_path.to_string(),
        hash,
    });

    let lock_json = serde_json::to_string_pretty(&lock_entries).unwrap_or_default();
    if let Err(e) = tokio::fs::write(lock_path, lock_json).await {
        return R::internal_error(format!("更新 lock file 失败: {}", e));
    }

    R::created(serde_json::json!({"name": body.skill_name, "source": body.source}))
}

// DELETE /api/skills/:name?work_dir=xxx
pub async fn uninstall_skill(
    State(_state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Query(query): Query<SkillQuery>,
) -> impl IntoResponse {
    let work_dir = PathBuf::from(&query.work_dir);
    let skills_dir = work_dir.join(".agents").join("skills").join(&name);
    let lock_path = work_dir.join("skills-lock.json");

    // 删除 skill 目录
    if skills_dir.exists() {
        if let Err(e) = tokio::fs::remove_dir_all(&skills_dir).await {
            return R::internal_error(format!("删除 skill 目录失败: {}", e));
        }
    }

    // 更新 lock file
    let mut lock_entries: Vec<SkillLockEntry> = match tokio::fs::read_to_string(&lock_path).await {
        Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    lock_entries.retain(|e| e.name != name);

    let lock_json = serde_json::to_string_pretty(&lock_entries).unwrap_or_default();
    if let Err(e) = tokio::fs::write(&lock_path, lock_json).await {
        return R::internal_error(format!("更新 lock file 失败: {}", e));
    }

    R::no_content()
}

// ─── Helpers ─────────────────────────────────────────────────────────

async fn parse_skill_frontmatter(path: &PathBuf) -> (String, Vec<String>) {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return (String::new(), Vec::new()),
    };

    // 解析 YAML frontmatter (--- ... ---)
    if !content.starts_with("---") {
        return (String::new(), Vec::new());
    }

    let rest = &content[3..];
    let end = match rest.find("---") {
        Some(i) => i,
        None => return (String::new(), Vec::new()),
    };

    let yaml_str = &rest[..end];
    let mut description = String::new();
    let mut allowed_tools: Vec<String> = Vec::new();

    for line in yaml_str.lines() {
        let line = line.trim();
        if let Some(desc) = line.strip_prefix("description:") {
            description = desc.trim().trim_matches('"').to_string();
        }
        if let Some(tools) = line.strip_prefix("allowed-tools:") {
            allowed_tools = tools
                .trim()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    (description, allowed_tools)
}

fn md5_hash(input: &str) -> u64 {
    // 简单 hash，不需要加密安全
    let mut hash: u64 = 0;
    for byte in input.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
    }
    hash
}

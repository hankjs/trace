pub mod events;
pub mod provider;
pub mod providers;

use std::collections::HashMap;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use provider::{CliProvider, ProviderSession};

/// Agent configuration persisted to disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub agent_type: String,
    pub binary_path: String,
}

/// A running session backed by a CLI provider.
pub struct ActiveSession {
    pub provider: Box<dyn CliProvider>,
    pub session: Mutex<ProviderSession>,
    pub cancel_token: CancellationToken,
    pub binary_path: String,
}

/// Global ACP state managed by Tauri.
pub struct AcpState {
    /// Active sessions keyed by session_id.
    pub sessions: RwLock<HashMap<String, ActiveSession>>,
    /// Configured agents.
    pub agents: RwLock<Vec<AgentConfig>>,
    /// Config file path.
    pub config_path: Mutex<String>,
}

impl AcpState {
    pub fn new(config_path: String) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            agents: RwLock::new(Vec::new()),
            config_path: Mutex::new(config_path),
        }
    }

    pub async fn load_config(&self) -> Result<(), String> {
        let path = self.config_path.lock().await.clone();
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(c) => c,
            Err(_) => return Ok(()), // No config file yet
        };
        let agents: Vec<AgentConfig> = serde_json::from_str(&content).map_err(|e| e.to_string())?;
        *self.agents.write().await = agents;
        Ok(())
    }

    pub async fn save_config(&self) -> Result<(), String> {
        let path = self.config_path.lock().await.clone();
        let agents = self.agents.read().await;
        let content = serde_json::to_string_pretty(&*agents).map_err(|e| e.to_string())?;
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| e.to_string())
    }
}

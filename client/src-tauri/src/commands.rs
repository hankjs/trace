use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::acp::events::{AcpEvent, AcpEventPayload};
use crate::acp::provider::ProviderSession;
use crate::acp::providers;
use crate::acp::{AcpState, ActiveSession, AgentConfig};

/// Create a new ACP session: register provider + session state (no process spawned yet).
#[tauri::command]
pub async fn acp_new_session(
    _app: AppHandle,
    state: State<'_, Arc<AcpState>>,
    agent_name: String,
    work_dir: String,
    session_id: String,
) -> Result<String, String> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.name == agent_name)
        .ok_or_else(|| format!("Agent '{}' not configured", agent_name))?
        .clone();
    drop(agents);

    let provider = providers::create_provider(&agent.agent_type)?;
    let provider_session = ProviderSession {
        work_dir,
        cli_session_id: None,
    };

    let active = ActiveSession {
        provider,
        session: tokio::sync::Mutex::new(provider_session),
        cancel_token: CancellationToken::new(),
        binary_path: agent.binary_path,
    };

    state
        .sessions
        .write()
        .await
        .insert(session_id.clone(), active);

    Ok(session_id)
}

/// Send a prompt to the local CLI agent.
#[tauri::command]
pub async fn acp_prompt(
    app: AppHandle,
    state: State<'_, Arc<AcpState>>,
    session_id: String,
    message: String,
) -> Result<(), String> {
    let sessions = state.sessions.read().await;
    let active = sessions.get(&session_id).ok_or("No active session")?;

    // Fresh cancel token for this prompt
    let cancel_token = CancellationToken::new();
    let binary_path = active.binary_path.clone();

    let (event_tx, mut event_rx) = mpsc::channel::<AcpEvent>(256);

    // We need to get a reference to the provider and session.
    // Since prompt takes &mut ProviderSession, we need to drop the read lock
    // and work with the session mutex directly.
    // But we can't hold the RwLock across an await, so we clone what we need.
    drop(sessions);

    // Forward events to Tauri frontend
    let app_clone = app.clone();
    let sid_clone = session_id.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let _ = app_clone.emit(
                "acp-event",
                &AcpEventPayload {
                    session_id: sid_clone.clone(),
                    event,
                },
            );
        }
    });

    // Run the prompt in a spawned task
    let state_inner = state.inner().clone();
    tokio::spawn(async move {
        let sessions = state_inner.sessions.read().await;
        let Some(active) = sessions.get(&session_id) else {
            let _ = event_tx
                .send(AcpEvent::Error {
                    message: "Session not found".to_string(),
                })
                .await;
            return;
        };

        let mut session_guard = active.session.lock().await;
        let result = active
            .provider
            .prompt(
                &binary_path,
                &message,
                &mut session_guard,
                event_tx.clone(),
                cancel_token,
            )
            .await;

        if let Err(e) = result {
            let _ = event_tx.send(AcpEvent::Error { message: e }).await;
        }
    });

    Ok(())
}

/// Cancel an in-progress prompt.
#[tauri::command]
pub async fn acp_cancel(state: State<'_, Arc<AcpState>>, session_id: String) -> Result<(), String> {
    let sessions = state.sessions.read().await;
    if let Some(active) = sessions.get(&session_id) {
        active.cancel_token.cancel();
    }
    Ok(())
}

/// Stop and remove the session.
#[tauri::command]
pub async fn acp_stop(state: State<'_, Arc<AcpState>>, session_id: String) -> Result<(), String> {
    let mut sessions = state.sessions.write().await;
    if let Some(active) = sessions.remove(&session_id) {
        active.cancel_token.cancel();
    }
    Ok(())
}

/// Get list of configured agents.
#[tauri::command]
pub async fn acp_get_agents(state: State<'_, Arc<AcpState>>) -> Result<Vec<AgentConfig>, String> {
    Ok(state.agents.read().await.clone())
}

/// Add a new agent configuration.
#[tauri::command]
pub async fn acp_add_agent(
    state: State<'_, Arc<AcpState>>,
    name: String,
    agent_type: String,
    binary_path: String,
) -> Result<(), String> {
    let config = AgentConfig {
        name,
        agent_type,
        binary_path,
    };
    {
        let mut agents = state.agents.write().await;
        agents.retain(|a| a.name != config.name);
        agents.push(config);
    }
    state.save_config().await
}

/// Remove an agent configuration.
#[tauri::command]
pub async fn acp_remove_agent(state: State<'_, Arc<AcpState>>, name: String) -> Result<(), String> {
    {
        let mut agents = state.agents.write().await;
        agents.retain(|a| a.name != name);
    }
    state.save_config().await
}

/// Test an agent: run version check to verify the binary works.
#[tauri::command]
pub async fn acp_test_agent(
    state: State<'_, Arc<AcpState>>,
    name: String,
) -> Result<String, String> {
    let agents = state.agents.read().await;
    let agent = agents
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("Agent '{}' not configured", name))?
        .clone();
    drop(agents);

    let provider = providers::create_provider(&agent.agent_type)?;
    let work_dir = std::env::temp_dir().to_string_lossy().to_string();

    let info = provider.test(&agent.binary_path, &work_dir).await?;

    let mut result = format!("Agent '{}' OK", agent.name);
    if let Some(version) = info.version {
        result = format!("{} (version: {})", result, version);
    }
    Ok(result)
}

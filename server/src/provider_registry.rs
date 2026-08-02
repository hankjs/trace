use hank_db::{Database, ProviderRecord};
use hank_provider::LlmProvider;
use std::collections::HashMap;
use std::sync::Arc;

/// Build an LlmProvider instance from a ProviderRecord.
pub fn build_provider_from_record(record: &ProviderRecord) -> Arc<dyn LlmProvider> {
    use hank_provider::anthropic::AnthropicProvider;
    use hank_provider::openai::OpenAiProvider;

    match record.provider_type.as_str() {
        "anthropic" => Arc::new(
            AnthropicProvider::new(record.api_key.clone()).with_base_url(record.base_url.clone()),
        ),
        _ => Arc::new(
            OpenAiProvider::new(record.api_key.clone())
                .with_base_url(record.base_url.clone())
                .with_name(record.name.clone()),
        ),
    }
}

/// Resolve a single provider by name from DB.
pub async fn resolve_provider(
    db: &Database,
    name: &str,
) -> Option<(ProviderRecord, Arc<dyn LlmProvider>)> {
    let record = db.get_provider_by_name(name).await.ok()??;
    if !record.enabled {
        return None;
    }
    let provider = build_provider_from_record(&record);
    Some((record, provider))
}

/// Get the model map from a ProviderRecord's JSON models field.
pub fn get_models_map(record: &ProviderRecord) -> HashMap<String, String> {
    serde_json::from_str(&record.models).unwrap_or_default()
}

/// Resolve model name using the provider record's model aliases.
pub fn resolve_model(record: &ProviderRecord, model_name: &str) -> String {
    let models = get_models_map(record);
    models
        .get(model_name)
        .cloned()
        .unwrap_or_else(|| model_name.to_string())
}

/// Resolve default model for a provider record.
pub fn resolve_default_model(record: &ProviderRecord) -> String {
    resolve_model(record, &record.default_model)
}

/// Get the default provider name (highest priority enabled provider).
pub async fn default_provider_name(db: &Database) -> Option<String> {
    let all = db.list_providers_ordered().await.unwrap_or_default();
    all.into_iter().find(|r| r.enabled).map(|r| r.name)
}

/// Resolve the highest-priority enabled provider.
pub async fn resolve_default(db: &Database) -> Option<(ProviderRecord, Arc<dyn LlmProvider>)> {
    let all = db.list_providers_ordered().await.unwrap_or_default();
    all.into_iter().find(|r| r.enabled).map(|r| {
        let provider = build_provider_from_record(&r);
        (r, provider)
    })
}

/// Returns an ordered list of providers for fallback: preferred first, then remaining by priority.
/// Only returns enabled providers.
pub async fn resolve_with_fallback(
    db: &Database,
    preferred_name: &str,
) -> Vec<(ProviderRecord, Arc<dyn LlmProvider>)> {
    let all = db.list_providers_ordered().await.unwrap_or_default();
    let mut result = Vec::new();

    // Put preferred first
    if let Some(pref) = all.iter().find(|r| r.name == preferred_name && r.enabled) {
        result.push((pref.clone(), build_provider_from_record(pref)));
    }

    // Then remaining enabled providers by priority
    for record in &all {
        if record.name == preferred_name || !record.enabled {
            continue;
        }
        result.push((record.clone(), build_provider_from_record(record)));
    }

    result
}

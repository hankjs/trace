use crate::response::{self as R};
use crate::AppState;
use axum::{
    extract::{Multipart, State},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct ImageGenRequest {
    pub prompt: String,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub size: Option<String>,
    pub quality: Option<String>,
    pub n: Option<u32>,
}

#[derive(Serialize)]
pub struct ImageGenResponse {
    pub images: Vec<ImageResult>,
    pub provider: String,
    pub model: String,
}

#[derive(Serialize)]
pub struct ImageResult {
    pub url: Option<String>,
    pub b64_json: Option<String>,
    pub revised_prompt: Option<String>,
}

pub async fn list_image_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let providers = state
        .db
        .list_image_providers_ordered()
        .await
        .unwrap_or_default();
    let list: Vec<serde_json::Value> = providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            let models: serde_json::Value =
                serde_json::from_str(&p.models).unwrap_or(serde_json::json!({}));
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "type": p.provider_type,
                "default_model": p.default_model,
                "models": models,
            })
        })
        .collect();
    R::ok(serde_json::json!({ "providers": list }))
}

// Text-to-image: JSON body
pub async fn generate_image(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ImageGenRequest>,
) -> impl IntoResponse {
    let (record, base_url, model) =
        match resolve_provider(&state, body.provider_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return R::bad_request(e),
        };
    let model = body.model.unwrap_or(model);

    let n = body.n.unwrap_or(1).min(4);
    let size = body.size.as_deref().unwrap_or("1024x1024");
    let quality = body.quality.as_deref().unwrap_or("standard");

    let req_body = serde_json::json!({
        "model": model,
        "prompt": body.prompt,
        "n": n,
        "size": size,
        "quality": quality,
        "response_format": "url",
    });

    let url = format!("{}/v1/images/generations", base_url);
    call_provider_json(&record.api_key, &url, req_body, &record.name, &model).await
}

// Image-to-image: multipart body (image file + prompt + params)
pub async fn edit_image(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let mut prompt = String::new();
    let mut provider_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut size = "1024x1024".to_string();
    let mut n: u32 = 1;
    let mut image_bytes: Option<(Vec<u8>, String)> = None; // (bytes, filename)

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "image" => {
                let filename = field.file_name().unwrap_or("image.png").to_string();
                let bytes: Vec<u8> = field.bytes().await.unwrap_or_default().to_vec();
                image_bytes = Some((bytes, filename));
            }
            "prompt" => prompt = field.text().await.unwrap_or_default(),
            "provider_id" => provider_id = Some(field.text().await.unwrap_or_default()),
            "model" => model = Some(field.text().await.unwrap_or_default()),
            "size" => size = field.text().await.unwrap_or_default(),
            "n" => {
                n = field
                    .text()
                    .await
                    .unwrap_or_default()
                    .parse::<u32>()
                    .unwrap_or(1)
                    .min(4)
            }
            _ => {}
        }
    }

    let (record, base_url, resolved_model) =
        match resolve_provider(&state, provider_id.as_deref()).await {
            Ok(v) => v,
            Err(e) => return R::bad_request(e),
        };
    let model = model.unwrap_or(resolved_model);

    let (img_bytes, img_name) = match image_bytes {
        Some(v) => v,
        None => return R::bad_request("Missing image field"),
    };

    // Build multipart for upstream
    let part = reqwest::multipart::Part::bytes(img_bytes)
        .file_name(img_name)
        .mime_str("image/png")
        .unwrap();

    let form = reqwest::multipart::Form::new()
        .part("image", part)
        .text("prompt", prompt)
        .text("model", model.clone())
        .text("n", n.to_string())
        .text("size", size)
        .text("response_format", "url");

    let url = format!("{}/v1/images/edits", base_url);
    let client = reqwest::Client::new();
    let resp = match client
        .post(&url)
        .bearer_auth(&record.api_key)
        .multipart(form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return R::internal_error(format!("Request failed: {e}")),
    };

    parse_image_response(resp, &record.name, &model).await
}

// --- helpers ---

async fn resolve_provider(
    state: &AppState,
    provider_id: Option<&str>,
) -> Result<(hank_db::ProviderRecord, String, String), &'static str> {
    let providers = state
        .db
        .list_image_providers_ordered()
        .await
        .unwrap_or_default();
    let record = if let Some(pid) = provider_id {
        providers
            .into_iter()
            .find(|p| p.enabled && (p.id == pid || p.name == pid))
    } else {
        providers.into_iter().find(|p| p.enabled)
    };
    let record = record.ok_or("No image provider configured or found")?;
    let base_url = if record.base_url.is_empty() {
        "https://api.openai.com".to_string()
    } else {
        record.base_url.trim_end_matches('/').to_string()
    };
    let model = record.default_model.clone();
    Ok((record, base_url, model))
}

async fn call_provider_json(
    api_key: &str,
    url: &str,
    body: serde_json::Value,
    provider_name: &str,
    model: &str,
) -> axum::response::Response {
    let client = reqwest::Client::new();
    let resp = match client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return R::internal_error(format!("Request failed: {e}")),
    };
    parse_image_response(resp, provider_name, model).await
}

async fn parse_image_response(
    resp: reqwest::Response,
    provider_name: &str,
    model: &str,
) -> axum::response::Response {
    let status = resp.status();
    let body_text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        tracing::error!("Image provider error {status}: {body_text}");
        return R::internal_error(format!("Provider error {status}: {body_text}"));
    }
    let json: serde_json::Value = match serde_json::from_str(&body_text) {
        Ok(v) => v,
        Err(e) => return R::internal_error(format!("Parse error: {e}")),
    };
    let images: Vec<ImageResult> = json["data"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|item| ImageResult {
            url: item["url"].as_str().map(|s| s.to_string()),
            b64_json: item["b64_json"].as_str().map(|s| s.to_string()),
            revised_prompt: item["revised_prompt"].as_str().map(|s| s.to_string()),
        })
        .collect();
    R::ok(ImageGenResponse {
        images,
        provider: provider_name.to_string(),
        model: model.to_string(),
    })
}

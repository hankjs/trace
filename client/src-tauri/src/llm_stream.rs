use serde::{Deserialize, Serialize};
use tauri::command;
use tauri::ipc::Channel;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStreamRequest {
    pub url: String,
    pub token: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SseEvent {
    pub data: String,
    pub done: bool,
}

/// Mock command: sends 10 events with 200ms delay each, to test real-time delivery.
#[command]
pub async fn llm_stream_test(on_event: Channel<SseEvent>) -> Result<(), String> {
    for i in 1..=10 {
        let _ = on_event.send(SseEvent {
            data: format!("mock event #{}", i),
            done: false,
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = on_event.send(SseEvent {
        data: String::new(),
        done: true,
    });
    Ok(())
}

/// Parse URL into (host, port, path)
fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let url = url::Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;
    let host = url.host_str().ok_or("No host in URL")?.to_string();
    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });
    let path = if let Some(q) = url.query() {
        format!("{}?{}", url.path(), q)
    } else {
        url.path().to_string()
    };
    Ok((host, port, path))
}

/// Tauri command: stream LLM completion via raw TCP, bypassing reqwest buffering.
/// Uses Channel for real-time event delivery directly to the calling frontend.
#[command]
pub async fn llm_stream(req: LlmStreamRequest, on_event: Channel<SseEvent>) -> Result<(), String> {
    let (host, port, path) = parse_url(&req.url)?;

    // Connect via raw TCP with NODELAY for real-time streaming
    let addr = format!("{}:{}", host, port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("TCP connect failed: {}", e))?;
    tcp.set_nodelay(true)
        .map_err(|e| format!("set_nodelay failed: {}", e))?;

    // Build raw HTTP/1.1 request (Connection: close so server closes after response)
    let http_req = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Authorization: Bearer {}\r\n\
         Accept: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        path,
        host,
        req.token,
        req.body.len(),
        req.body
    );

    let (reader, mut writer) = tcp.into_split();
    writer
        .write_all(http_req.as_bytes())
        .await
        .map_err(|e| format!("Write failed: {}", e))?;

    let mut buf_reader = BufReader::new(reader);
    let mut line_buf = String::new();

    // Skip HTTP response headers
    loop {
        line_buf.clear();
        let n = buf_reader
            .read_line(&mut line_buf)
            .await
            .map_err(|e| format!("Read header failed: {}", e))?;
        if n == 0 {
            return Err("Connection closed during headers".into());
        }
        if line_buf.trim().is_empty() {
            break;
        } // End of headers
    }

    // Read SSE lines in real-time
    loop {
        line_buf.clear();
        let n = buf_reader
            .read_line(&mut line_buf)
            .await
            .map_err(|e| format!("Read failed: {}", e))?;
        if n == 0 {
            break;
        } // EOF

        let line = line_buf.trim_end();
        if line.starts_with("data: ") {
            let data = line[6..].to_string();
            // Detect end-of-stream marker and break early (don't wait for TCP close)
            let is_end = data.contains("\"type\":\"message_end\"");
            let _ = on_event.send(SseEvent { data, done: false });
            if is_end {
                break;
            }
        }
    }

    let _ = on_event.send(SseEvent {
        data: String::new(),
        done: true,
    });
    Ok(())
}

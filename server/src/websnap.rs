//! 网页截图：用 chromiumoxide 驱动本机安装的 Chrome/Chromium（headless）截全页 PNG。
//!
//! Chrome 路径解析顺序：config.toml 的 `chrome_path` → `CHROME_PATH` 环境变量 → 常见路径自动探测
//! （macOS `/Applications/Google Chrome.app/...`；Linux PATH 里的 chromium/chromium-browser/google-chrome）。

use anyhow::{anyhow, bail, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::viewport::Viewport;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use std::time::Duration;

/// 截图整体超时（含 Chrome 启动、页面加载）
const SNAP_TIMEOUT: Duration = Duration::from_secs(30);
/// 视口宽度（高度只影响首屏，截图走 full_page）
const VIEWPORT_W: u32 = 1280;
const VIEWPORT_H: u32 = 800;

/// 截图入口：URL 校验 → 启动 headless Chrome → 全页截图 PNG
pub async fn snap_url(chrome_path: Option<&str>, url: &str) -> Result<Vec<u8>> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("仅支持 http/https 链接");
    }
    let chrome = resolve_chrome_path(chrome_path)?;
    match tokio::time::timeout(SNAP_TIMEOUT, snap_inner(&chrome, url)).await {
        Ok(r) => r,
        Err(_) => Err(anyhow!(
            "网页截图超时（>{}s）：{url}",
            SNAP_TIMEOUT.as_secs()
        )),
    }
}

fn resolve_chrome_path(chrome_path: Option<&str>) -> Result<String> {
    if let Some(p) = chrome_path.map(str::trim).filter(|p| !p.is_empty()) {
        return Ok(p.to_string());
    }
    if let Ok(p) = std::env::var("CHROME_PATH") {
        if !p.trim().is_empty() {
            return Ok(p.trim().to_string());
        }
    }
    let macos = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
    if std::path::Path::new(macos).exists() {
        return Ok(macos.to_string());
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        if let Some(p) = find_in_path(name) {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "server 未安装 Chrome，请在 config.toml 配置 chrome_path"
    ))
}

fn find_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

async fn snap_inner(chrome_path: &str, url: &str) -> Result<Vec<u8>> {
    let config = BrowserConfig::builder()
        .chrome_executable(chrome_path)
        // 服务器常以 root/容器运行，Chrome 需要 --no-sandbox 才能启动
        .no_sandbox()
        .arg("--disable-gpu")
        .arg("--hide-scrollbars")
        .viewport(Some(Viewport {
            width: VIEWPORT_W,
            height: VIEWPORT_H,
            device_scale_factor: None,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        }))
        .build()
        .map_err(|e| anyhow!(e))?;
    let (mut browser, mut handler) = Browser::launch(config).await?;
    // DevTools 连接的事件循环，必须持续驱动否则所有请求都会卡住
    let driver = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let result = async {
        let page = browser.new_page(url).await?;
        page.wait_for_navigation().await?;
        let png = page
            .screenshot(ScreenshotParams::builder().full_page(true).build())
            .await?;
        Ok::<_, anyhow::Error>(png)
    }
    .await;
    let _ = browser.close().await;
    drop(browser);
    driver.abort();
    result
}

#[cfg(test)]
mod tests {
    /// 本机实测（需要已安装 Chrome）：
    /// cargo test -p hank-server websnap -- --ignored
    #[tokio::test]
    #[ignore]
    async fn snap_example_com() {
        let png = super::snap_url(None, "https://example.com").await.unwrap();
        assert!(png.len() > 1000);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        std::fs::write("/tmp/websnap_example.png", &png).unwrap();
        eprintln!("wrote /tmp/websnap_example.png ({} bytes)", png.len());
    }

    #[tokio::test]
    async fn reject_non_http_url() {
        assert!(super::snap_url(None, "ftp://example.com").await.is_err());
        assert!(super::snap_url(None, "example.com").await.is_err());
    }
}

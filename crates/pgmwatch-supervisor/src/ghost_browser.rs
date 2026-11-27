use crate::win32::WindowsJobGuard;
use futures_util::{SinkExt, StreamExt};
use pgmwatch_core::models::Platform;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub fn detect_msedge_path(configured: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files (x86)"));

    resolve_chromium_browser(
        configured,
        exe_dir.as_deref(),
        local_app_data.as_deref(),
        &[&program_files_x86, &program_files],
    )
}

fn resolve_chromium_browser(
    configured: &str,
    exe_dir: Option<&Path>,
    local_app_data: Option<&Path>,
    program_dirs: &[&Path],
) -> Option<PathBuf> {
    if !configured.trim().is_empty() {
        let p = PathBuf::from(configured.trim());
        if p.exists() {
            return Some(p);
        }
    }

    if let Some(dir) = exe_dir {
        for rel in [
            "bin/browser/msedge.exe",
            "bin/browser/chrome.exe",
            "bin/browser/chromium.exe",
            "bin/browser/brave.exe",
            "browser/msedge.exe",
            "browser/chrome.exe",
        ] {
            let p = dir.join(rel);
            if p.exists() {
                return Some(p);
            }
        }
    }

    let browser_subpaths = [
        r"Microsoft\Edge\Application\msedge.exe",
        r"Google\Chrome\Application\chrome.exe",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
    ];
    for sub in browser_subpaths {
        for root in program_dirs {
            let p = root.join(sub);
            if p.exists() {
                return Some(p);
            }
        }
        if let Some(local) = local_app_data {
            let p = local.join(sub);
            if p.exists() {
                return Some(p);
            }
        }
    }

    None
}

#[derive(Debug, Clone)]
pub struct SniffedStream {
    pub stream_url: String,
    pub headers: Vec<(String, String)>,
}

fn build_headless_edge_args(port: u16, profile_dir: &Path, target_url: &str) -> Vec<String> {
    vec![
        "--headless=new".to_string(),
        "--disable-gpu".to_string(),
        "--disable-background-mode".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--edge-skip-compat-layer-relaunch".to_string(),
        "--disk-cache-size=1".to_string(),
        "--media-cache-size=1".to_string(),
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", profile_dir.display()),
        target_url.to_string(),
    ]
}

fn prune_ephemeral_edge_caches(profile_dir: &Path) {
    if profile_dir.file_name().and_then(|s| s.to_str()) != Some("mode_b_ephemeral") {
        return;
    }
    for rel in [
        "ShaderCache",
        "GrShaderCache",
        "GraphiteDawnCache",
        "Default/Cache",
        "Default/Code Cache",
        "Default/GPUCache",
        "Default/DawnGraphiteCache",
        "Default/DawnWebGPUCache",
    ] {
        let p = profile_dir.join(rel);
        if p.exists() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

/// Mode A: 3-second headless Edge CDP sniff capturing .m3u8 / .mpd / .flv + guest headers.
pub async fn sniff_media_via_edge_cdp(
    msedge: &Path,
    profile_dir: &Path,
    page_url: &str,
    job_guard: Arc<WindowsJobGuard>,
) -> anyhow::Result<SniffedStream> {
    std::fs::create_dir_all(profile_dir)?;
    let port = pick_ephemeral_port();
    let edge_args = build_headless_edge_args(port, profile_dir, page_url);
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(msedge);
    cmd.no_window()
        .args(&edge_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child: Child = cmd.spawn()?;

    let _ = job_guard.assign_pid(child.id());

    let result = tokio::time::timeout(Duration::from_secs(7), async {
        tokio::time::sleep(Duration::from_millis(900)).await;
        let targets_url = format!("http://127.0.0.1:{port}/json");
        let targets: serde_json::Value = reqwest::get(&targets_url).await?.json().await?;
        let ws_url = targets
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|t| t.get("webSocketDebuggerUrl"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("No CDP target ws url found"))?
            .to_string();

        let (mut ws, _) = connect_async(&ws_url).await?;
        ws.send(Message::Text(
            json!({ "id": 1, "method": "Network.enable", "params": {} })
                .to_string()
                .into(),
        ))
        .await?;
        ws.send(Message::Text(
            json!({ "id": 2, "method": "Page.navigate", "params": { "url": page_url } })
                .to_string()
                .into(),
        ))
        .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(Ok(Message::Text(txt)))) =
                tokio::time::timeout(Duration::from_millis(400), ws.next()).await
                && let Ok(ev) = serde_json::from_str::<serde_json::Value>(&txt)
                && ev.get("method").and_then(|m| m.as_str()) == Some("Network.requestWillBeSent")
                && let Some(req_url) = ev
                    .pointer("/params/request/url")
                    .and_then(|u| u.as_str())
            {
                let lower = req_url.to_ascii_lowercase();
                if lower.contains(".m3u8")
                    || lower.contains(".mpd")
                    || lower.contains(".flv")
                {
                    let mut headers = Vec::new();
                    if let Some(hmap) = ev.pointer("/params/request/headers").and_then(|v| v.as_object()) {
                        for (k, v) in hmap {
                            if matches!(
                                k.to_ascii_lowercase().as_str(),
                                "user-agent" | "referer" | "origin"
                            ) && let Some(s) = v.as_str()
                            {
                                headers.push((k.clone(), s.to_string()));
                            }
                        }
                    }
                    return Ok(SniffedStream {
                        stream_url: req_url.to_string(),
                        headers,
                    });
                }
            }
        }
        Err(anyhow::anyhow!("Edge CDP Mode A did not capture manifest within window"))
    })
    .await;

    let _ = child.kill();
    let _ = child.wait();
    prune_ephemeral_edge_caches(profile_dir);
    result?
}

/// Opens a visible Edge window bound strictly to `data/profiles/<platform>_auth` for optional burner login fallback.
/// Enables a local loopback CDP port saved to `.cdp_port` so Sync Session can export cookies directly
/// from the open login window (and close it cleanly) without ever colliding on the Windows profile lockfile.
pub fn open_isolated_login_window(
    msedge: &Path,
    auth_profile_dir: &Path,
    login_url: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(auth_profile_dir)?;
    let port = pick_ephemeral_port();
    let _ = std::fs::write(auth_profile_dir.join(".cdp_port"), port.to_string());
    Command::new(msedge)
        .args([
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-mode",
            "--edge-skip-compat-layer-relaunch",
            "--disk-cache-size=1",
            "--media-cache-size=1",
            &format!("--remote-debugging-port={port}"),
            &format!("--user-data-dir={}", auth_profile_dir.display()),
            login_url,
        ])
        .spawn()?;
    Ok(())
}

pub fn wipe_auth_profile(auth_profile_dir: &Path) -> anyhow::Result<()> {
    if auth_profile_dir.exists() {
        std::fs::remove_dir_all(auth_profile_dir)?;
    }
    std::fs::create_dir_all(auth_profile_dir)?;
    Ok(())
}

async fn try_export_cookies_from_cdp_port(
    port: u16,
    cookies_out_path: &Path,
    close_browser: bool,
) -> anyhow::Result<usize> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap_or_default();

    let mut ws_url = None;
    let mut use_browser_target = false;
    let version_url = format!("http://127.0.0.1:{port}/json/version");
    if let Ok(resp) = http.get(&version_url).send().await
        && let Ok(ver) = resp.json::<serde_json::Value>().await
        && let Some(u) = ver.get("webSocketDebuggerUrl").and_then(|v| v.as_str())
    {
        ws_url = Some(u.to_string());
        use_browser_target = true;
    } else {
        let targets_url = format!("http://127.0.0.1:{port}/json");
        if let Ok(resp) = http.get(&targets_url).send().await
            && let Ok(targets) = resp.json::<serde_json::Value>().await
            && let Some(u) = targets
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(|t| t.get("webSocketDebuggerUrl"))
                .and_then(|v| v.as_str())
        {
            ws_url = Some(u.to_string());
        }
    }

    let ws_url = ws_url.ok_or_else(|| anyhow::anyhow!("CDP port {port} not responding"))?;
    let (mut ws, _) = connect_async(&ws_url).await?;
    let method = if use_browser_target {
        "Storage.getCookies"
    } else {
        "Network.getAllCookies"
    };
    ws.send(Message::Text(
        json!({ "id": 1, "method": method, "params": {} })
            .to_string()
            .into(),
    ))
    .await?;

    let mut exported: Option<usize> = None;
    while let Some(Ok(Message::Text(txt))) = ws.next().await {
        if let Ok(ev) = serde_json::from_str::<serde_json::Value>(&txt)
            && ev.get("id").and_then(|v| v.as_i64()) == Some(1)
        {
            let cookies = ev
                .pointer("/result/cookies")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let netscape = format_netscape_cookies(&cookies);
            if let Some(parent) = cookies_out_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(cookies_out_path, netscape)?;
            exported = Some(cookies.len());
            break;
        }
    }
    if close_browser {
        let _ = ws
            .send(Message::Text(
                json!({ "id": 2, "method": "Browser.close", "params": {} })
                    .to_string()
                    .into(),
            ))
            .await;
    }
    exported.ok_or_else(|| anyhow::anyhow!("Did not receive cookies from Edge profile"))
}

/// Exports cookies from PGMWatch's isolated visible Sign-In profile (`data/profiles/<platform>_auth`)
/// into a Netscape `cookies.txt` file for authenticated `yt-dlp` and stats requests.
pub async fn export_cookies_from_auth_profile(
    msedge: &Path,
    auth_profile_dir: &Path,
    cookies_out_path: &Path,
    job_guard: Arc<WindowsJobGuard>,
) -> anyhow::Result<usize> {
    if !auth_profile_dir.join("Default").exists() {
        anyhow::bail!("No signed-in Edge profile exists yet");
    }

    // Fast path: if the visible login Edge window is still open with our loopback CDP port,
    // export cookies directly from it and close it cleanly (zero profile lockfile conflict).
    let port_file = auth_profile_dir.join(".cdp_port");
    if let Ok(raw_port) = std::fs::read_to_string(&port_file)
        && let Ok(existing_port) = raw_port.trim().parse::<u16>()
        && existing_port > 0
        && let Ok(Ok(count)) = tokio::time::timeout(
            Duration::from_secs(5),
            try_export_cookies_from_cdp_port(existing_port, cookies_out_path, true),
        )
        .await
    {
        let _ = std::fs::remove_file(&port_file);
        return Ok(count);
    }

    let port = pick_ephemeral_port();
    let edge_args = build_headless_edge_args(port, auth_profile_dir, "about:blank");
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(msedge);
    cmd.no_window()
        .args(&edge_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child: Child = cmd.spawn()?;

    // Use a per-export Job Object so dropping it atomically terminates the entire headless Edge
    // process tree (browser + renderer + storage utility processes) and never leaves a zombie
    // headless Edge holding the profile lockfile (which would also swallow future Sign In windows).
    let export_job = WindowsJobGuard::new(0).ok();
    if let Some(ref ej) = export_job {
        let _ = ej.assign_pid(child.id());
    } else {
        let _ = job_guard.assign_pid(child.id());
    }

    let res = tokio::time::timeout(Duration::from_secs(12), async {
        for _ in 0..36 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(Some(status)) = child.try_wait() {
                anyhow::bail!(
                    "Edge profile is locked by another Edge process ({status}). Click Sign In again, log in, then click Sync Session while the window is open."
                );
            }
            if let Ok(count) =
                try_export_cookies_from_cdp_port(port, cookies_out_path, true).await
            {
                tokio::time::sleep(Duration::from_millis(150)).await;
                return Ok(count);
            }
        }
        anyhow::bail!("Timed out waiting for Edge CDP cookie export")
    })
    .await;

    let _ = child.kill();
    let _ = child.wait();
    drop(export_job);
    res?
}

fn sanitize_cookie_field(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != '\r' && *c != '\n' && *c != '\0' && *c != '\t')
        .collect()
}

fn format_netscape_cookies(cookies: &[serde_json::Value]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        let domain = sanitize_cookie_field(c.get("domain").and_then(|v| v.as_str()).unwrap_or(""));
        let name = sanitize_cookie_field(c.get("name").and_then(|v| v.as_str()).unwrap_or(""));
        let value = sanitize_cookie_field(c.get("value").and_then(|v| v.as_str()).unwrap_or(""));
        let path = sanitize_cookie_field(c.get("path").and_then(|v| v.as_str()).unwrap_or("/"));
        if domain.is_empty() || name.is_empty() {
            continue;
        }
        let include_sub = if domain.starts_with('.') { "TRUE" } else { "FALSE" };
        let secure = if c.get("secure").and_then(|v| v.as_bool()).unwrap_or(false) {
            "TRUE"
        } else {
            "FALSE"
        };
        let expires = c
            .get("expires")
            .and_then(|v| v.as_f64())
            .map(|e| if e <= 0.0 { 0i64 } else { e.round() as i64 })
            .unwrap_or(0);
        out.push_str(&format!(
            "{domain}\t{include_sub}\t{path}\t{secure}\t{expires}\t{name}\t{value}\n"
        ));
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct DomStatsSnapshot {
    pub ccv: Option<f64>,
    pub total_views: Option<f64>,
    pub likes: Option<f64>,
}

pub fn parse_compact_count(raw: &str) -> Option<f64> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && !bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx >= bytes.len() {
        return None;
    }
    let mut num_buf = String::new();
    let mut saw_dot = false;
    while idx < bytes.len() {
        let ch = bytes[idx] as char;
        if ch.is_ascii_digit() {
            num_buf.push(ch);
        } else if ch == ',' {
            // skip thousands separator
        } else if ch == '.' && !saw_dot {
            saw_dot = true;
            num_buf.push(ch);
        } else {
            break;
        }
        idx += 1;
    }
    let base: f64 = num_buf.parse().ok()?;
    let rest = trimmed[idx..].trim_start();
    let mult = if let Some(first) = rest.chars().next() {
        match first.to_ascii_uppercase() {
            'K' => 1_000.0,
            'M' => 1_000_000.0,
            'B' => 1_000_000_000.0,
            _ => 1.0,
        }
    } else {
        1.0
    };
    let val = base * mult;
    if val.is_finite() { Some(val) } else { None }
}

/// Mode B: Headless Edge DOM stats scraper for Bigo, Custom, and fallback Facebook pages.
pub async fn scrape_dom_stats_via_edge_cdp(
    msedge: &Path,
    profile_dir: &Path,
    page_url: &str,
    platform: Platform,
    job_guard: Arc<WindowsJobGuard>,
) -> anyhow::Result<DomStatsSnapshot> {
    std::fs::create_dir_all(profile_dir)?;
    let port = pick_ephemeral_port();
    let edge_args = build_headless_edge_args(port, profile_dir, page_url);
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(msedge);
    cmd.no_window()
        .args(&edge_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child: Child = cmd.spawn()?;

    let _ = job_guard.assign_pid(child.id());

    let plat_tag = match platform {
        Platform::Bigo => "bigo",
        Platform::Facebook => "facebook",
        _ => "custom",
    };

    let result = tokio::time::timeout(Duration::from_secs(9), async {
        let mut ws_url = None;
        for _ in 0..18 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let targets_url = format!("http://127.0.0.1:{port}/json");
            if let Ok(resp) = reqwest::get(&targets_url).await
                && let Ok(targets) = resp.json::<serde_json::Value>().await
                && let Some(u) = targets
                    .as_array()
                    .and_then(|arr| arr.iter().find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page")))
                    .and_then(|t| t.get("webSocketDebuggerUrl"))
                    .and_then(|v| v.as_str())
            {
                ws_url = Some(u.to_string());
                break;
            }
        }
        let ws_url = ws_url.ok_or_else(|| anyhow::anyhow!("No CDP page target for Mode B"))?;
        let (mut ws, _) = connect_async(&ws_url).await?;
        tokio::time::sleep(Duration::from_millis(2200)).await;

        let js = format!(
            r#"(() => {{
                const plat = {plat_json};
                const html = document.documentElement ? document.documentElement.innerHTML : "";
                const pickSel = (sels) => {{
                    for (const s of sels) {{
                        const el = document.querySelector(s);
                        if (el && el.textContent && /\d/.test(el.textContent)) return el.textContent.trim();
                    }}
                    return null;
                }};
                let ccv = null, views = null, likes = null;
                if (plat === "bigo") {{
                    const m = html.match(/"(?:room_user_count|audienceCount|viewerCount|user_count)"\s*:\s*(\d+)/i);
                    ccv = m ? m[1] : pickSel(['[class*="viewer"]', '[class*="audience"]', '[class*="online-num"]']);
                    const lm = html.match(/"(?:like_count|likes)"\s*:\s*(\d+)/i);
                    if (lm) likes = lm[1];
                }} else if (plat === "facebook") {{
                    const m = html.match(/"(?:live_viewer_count_read_only|live_viewer_count|concurrent_view_count|viewer_count)"\s*:\s*(\d+)/i);
                    if (m) ccv = m[1];
                    if (!ccv) {{
                        const el = document.querySelector('[aria-label*="watching"]');
                        if (el) ccv = el.getAttribute('aria-label') || el.textContent;
                    }}
                    if (!ccv) {{
                        const mTxt = (document.body?.innerText || "").match(/([\d,.]+[KMB]?)\s+watching\s+now/i);
                        if (mTxt) ccv = mTxt[1];
                    }}
                    const vm = html.match(/"(?:play_count|video_view_count)"\s*:\s*(\d+)/i);
                    if (vm) views = vm[1];
                }} else {{
                    const ld = Array.from(document.querySelectorAll('script[type="application/ld+json"]')).map(s => s.textContent).join(" ");
                    const mLd = ld.match(/"userInteractionCount"\s*:\s*"?(\d+)/i);
                    ccv = mLd ? mLd[1] : pickSel(['[data-testid*="viewer"]', '[class*="viewer"]', '[class*="watching"]']);
                }}
                return {{ ccv, views, likes }};
            }})()"#,
            plat_json = serde_json::to_string(plat_tag)?
        );

        ws.send(Message::Text(
            json!({
                "id": 10,
                "method": "Runtime.evaluate",
                "params": { "expression": js, "returnByValue": true }
            })
            .to_string()
            .into(),
        ))
        .await?;

        while let Some(Ok(Message::Text(txt))) = ws.next().await {
            if let Ok(ev) = serde_json::from_str::<serde_json::Value>(&txt)
                && ev.get("id").and_then(|v| v.as_i64()) == Some(10)
            {
                let val = ev.pointer("/result/result/value");
                let parse_field = |k: &str| -> Option<f64> {
                    let v = val?.get(k)?;
                    if let Some(n) = v.as_f64() {
                        Some(n)
                    } else if let Some(s) = v.as_str() {
                        parse_compact_count(s)
                    } else {
                        None
                    }
                };
                return Ok(DomStatsSnapshot {
                    ccv: parse_field("ccv"),
                    total_views: parse_field("views"),
                    likes: parse_field("likes"),
                });
            }
        }
        Err(anyhow::anyhow!("Edge CDP Mode B returned no evaluation payload"))
    })
    .await;

    let _ = child.kill();
    let _ = child.wait();
    prune_ephemeral_edge_caches(profile_dir);
    result?
}

fn pick_ephemeral_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9229)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_netscape_cookies_sanitizes_control_chars() {
        let cookies = vec![json!({
            "domain": ".youtube.com",
            "name": "SID",
            "value": "abc\r\nforged\tval",
            "path": "/",
            "secure": true,
            "expires": 1800000000.4
        })];
        let txt = format_netscape_cookies(&cookies);
        assert!(txt.contains(".youtube.com\tTRUE\t/\tTRUE\t1800000000\tSID\tabcforgedval\n"));
        assert_eq!(txt.lines().count(), 2);
    }

    #[test]
    fn test_parse_compact_count_formats() {
        assert_eq!(parse_compact_count("12,345"), Some(12345.0));
        assert_eq!(parse_compact_count("1.2K"), Some(1200.0));
        assert_eq!(parse_compact_count("3.4M"), Some(3_400_000.0));
        assert_eq!(parse_compact_count("42 watching now"), Some(42.0));
        assert_eq!(parse_compact_count("980 viewers"), Some(980.0));
        assert_eq!(parse_compact_count("no viewers yet"), None);
    }

    #[test]
    fn test_headless_edge_args_bound_disk_and_media_cache() {
        let args = build_headless_edge_args(9222, Path::new("data/profiles/mode_b_ephemeral"), "https://example.com");
        assert!(args.contains(&"--disk-cache-size=1".to_string()));
        assert!(args.contains(&"--media-cache-size=1".to_string()));
        assert!(args.contains(&"--remote-debugging-port=9222".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("https://example.com"));
    }

    #[test]
    fn test_resolve_chromium_browser_prefers_configured_then_portable() {
        let tmp = std::env::temp_dir().join(format!("pgmwatch-browser-{}", uuid::Uuid::new_v4()));
        let exe_dir = tmp.join("app");
        let prog_dir = tmp.join("pf");
        let portable_chrome = exe_dir.join("bin").join("browser").join("chrome.exe");
        let host_edge = prog_dir.join("Microsoft").join("Edge").join("Application").join("msedge.exe");
        let explicit_exe = tmp.join("custom.exe");

        std::fs::create_dir_all(portable_chrome.parent().unwrap()).unwrap();
        std::fs::create_dir_all(host_edge.parent().unwrap()).unwrap();
        std::fs::write(&portable_chrome, b"stub").unwrap();
        std::fs::write(&host_edge, b"stub").unwrap();
        std::fs::write(&explicit_exe, b"stub").unwrap();

        assert_eq!(
            resolve_chromium_browser(explicit_exe.to_str().unwrap(), Some(&exe_dir), None, &[&prog_dir]),
            Some(explicit_exe)
        );
        assert_eq!(
            resolve_chromium_browser("", Some(&exe_dir), None, &[&prog_dir]),
            Some(portable_chrome.clone())
        );
        let _ = std::fs::remove_file(&portable_chrome);
        assert_eq!(
            resolve_chromium_browser("", Some(&exe_dir), None, &[&prog_dir]),
            Some(host_edge)
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

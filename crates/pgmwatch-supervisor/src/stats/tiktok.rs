use super::facebook::extract_json_field_number;
use super::{unavailable_stats, ChannelStatsUpdate};
use pgmwatch_core::models::TriStateMetric;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

pub(super) fn parse_tiktok_public_ccv(html: &str) -> Option<f64> {
    for key in [
        "\"userCount\"",
        "\"user_count\"",
        "\"viewerCount\"",
        "\"viewer_count\"",
    ] {
        let mut rest = html;
        while let Some(idx) = rest.find(key) {
            let after_key = &rest[idx + key.len()..];
            let trimmed = after_key.trim_start();
            if let Some(after_colon) = trimmed.strip_prefix(':') {
                let num_start = after_colon.trim_start();
                let digits: String = num_start
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(val) = digits.parse::<f64>()
                    && val.is_finite()
                {
                    return Some(val);
                }
            }
            rest = after_key;
        }
    }
    None
}

pub(super) async fn fetch_tiktok_public_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
) -> ChannelStatsUpdate {
    let Some(username) = extract_tiktok_username(url) else {
        return unavailable_stats(channel_id, "TikTok sidecar awaiting room connection");
    };
    let live_url = format!("https://www.tiktok.com/@{username}/live");
    let html = match client
        .get(&live_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
        _ => return unavailable_stats(channel_id, "TikTok sidecar reconnecting"),
    };

    if let Some(ccv) = parse_tiktok_public_ccv(&html) {
        let total_views = ["totalUser", "total_user", "enterCount", "enter_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok total views awaiting sidecar"));
        let likes = ["likeCount", "like_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok likes awaiting sidecar"));
        let shares = ["shareCount", "share_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok shares awaiting sidecar"));
        ChannelStatsUpdate {
            channel_id: channel_id.to_string(),
            ccv: TriStateMetric::value(ccv, 100.0),
            total_views,
            likes,
            chat_rate_per_min: TriStateMetric::unavailable("TikTok sidecar reconnecting"),
            shares,
            gifts_count: TriStateMetric::unavailable("TikTok gifts awaiting sidecar"),
            extras: HashMap::new(),
        }
    } else {
        unavailable_stats(channel_id, "TikTok sidecar awaiting room connection")
    }
}

pub(crate) fn extract_tiktok_username(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = reqwest::Url::parse(&normalized).ok()?;
    let segments: Vec<&str> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(at_seg) = segments.iter().find(|s| s.starts_with('@')) {
        let user = at_seg.trim_start_matches('@').trim();
        if !user.is_empty() {
            return Some(user.to_string());
        }
    }
    let first = segments.first()?.trim_start_matches('@').trim();
    if first.is_empty()
        || matches!(
            first.to_ascii_lowercase().as_str(),
            "t" | "v" | "live" | "video" | "embed" | "share" | "explore" | "following"
        )
    {
        return None;
    }
    Some(first.to_string())
}


#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidecarEvent {
    Stats {
        channel_id: String,
        ccv: Option<f64>,
        #[serde(default)]
        total_views: Option<f64>,
        likes: Option<f64>,
        #[serde(default)]
        shares: Option<f64>,
        gifts: Option<f64>,
        chat_count: Option<f64>,
    },
    Chat {
        channel_id: String,
        viewer_id: String,
        text: String,
    },
    Error {
        channel_id: String,
        message: String,
    },
}

pub struct TikTokSidecarClient {
    stdin: Option<tokio::process::ChildStdin>,
    child: Option<tokio::process::Child>,
}

impl TikTokSidecarClient {
    pub fn spawn(
        node_exe: &Path,
        script_path: &Path,
        on_event: tokio::sync::mpsc::Sender<SidecarEvent>,
        job_guard: Option<&crate::win32::WindowsJobGuard>,
    ) -> Self {
        if !node_exe.exists() || !script_path.exists() {
            tracing::info!("TikTok sidecar skipped (node.exe or sidecar script not staged yet)");
            return Self { stdin: None, child: None };
        }
        use pgmwatch_core::process::BackgroundCommandExt;
        let mut cmd = Command::new(node_exe);
        cmd.no_window()
            .kill_on_drop(true)
            .arg(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                if let (Some(guard), Some(pid)) = (job_guard, child.id())
                    && let Err(err) = guard.assign_pid(pid)
                {
                    tracing::debug!("Failed assigning TikTok sidecar PID to job guard: {err}");
                }
                let stdin = child.stdin.take();
                if let Some(stderr) = child.stderr.take() {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                tracing::warn!("tiktok-sidecar: {trimmed}");
                            }
                        }
                    });
                }
                if let Some(stdout) = child.stdout.take() {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(ev) = serde_json::from_str::<SidecarEvent>(&line) {
                                let _ = on_event.send(ev).await;
                            }
                        }
                    });
                }
                Self {
                    stdin,
                    child: Some(child),
                }
            }
            Err(err) => {
                tracing::warn!("Failed to spawn TikTok sidecar: {err}");
                Self { stdin: None, child: None }
            }
        }
    }

    pub async fn stop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    pub async fn subscribe(&mut self, channel_id: &str, username: &str, session_id: Option<&str>) {
        if let Some(ref mut stdin) = self.stdin {
            let line = serde_json::json!({
                "cmd": "subscribe",
                "channel_id": channel_id,
                "username": username,
                "session_id": session_id
            })
            .to_string();
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            let _ = stdin.flush().await;
        }
    }

    pub async fn unsubscribe(&mut self, channel_id: &str) {
        if let Some(ref mut stdin) = self.stdin {
            let line = serde_json::json!({
                "cmd": "unsubscribe",
                "channel_id": channel_id
            })
            .to_string();
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            let _ = stdin.flush().await;
        }
    }
}


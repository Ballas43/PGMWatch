use crate::stats::extract_twitch_login;
use crate::supervisor::{SupervisorState, WsBroadcast};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::models::{AlertState, FaultKind, IncidentRecord, Platform};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::AbortHandle;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

pub struct ChatRateTracker {
    events: HashMap<String, VecDeque<Instant>>,
}

impl Default for ChatRateTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatRateTracker {
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    pub fn record(&mut self, channel_id: &str, now: Instant) {
        let q = self.events.entry(channel_id.to_string()).or_default();
        q.push_back(now);
        Self::prune(q, now);
    }

    pub fn rate_per_min(&mut self, channel_id: &str, now: Instant) -> Option<f64> {
        let q = self.events.get_mut(channel_id)?;
        Self::prune(q, now);
        Some(q.len() as f64)
    }

    fn prune(q: &mut VecDeque<Instant>, now: Instant) {
        let window = Duration::from_secs(60);
        while let Some(front) = q.front() {
            if now.duration_since(*front) > window {
                q.pop_front();
            } else {
                break;
            }
        }
    }
}

pub fn parse_twitch_irc_privmsg(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    // Strip optional IRCv3 `@tag=val;... ` prefix requested via `CAP REQ :twitch.tv/tags`.
    let body = if let Some(rest) = trimmed.strip_prefix('@') {
        rest.split_once(' ')?.1.trim_start()
    } else {
        trimmed
    };
    if !body.starts_with(':') {
        return None;
    }
    let bang = body.find('!')?;
    let viewer_id = body[1..bang].trim();
    if viewer_id.is_empty() {
        return None;
    }
    let priv_idx = body.find(" PRIVMSG #")?;
    let after_priv = &body[priv_idx + " PRIVMSG #".len()..];
    let colon_idx = after_priv.find(" :")?;
    let text = after_priv[colon_idx + 2..].trim();
    if text.is_empty() {
        return None;
    }
    Some((viewer_id.to_string(), text.to_string()))
}

/// Counts live Twitch gift events from IRCv3 tagged lines:
/// - `USERNOTICE` with `msg-id=subgift` or `msg-id=anonsubgift` -> 1 gifted sub
///   (we skip `submysterygift` because Twitch immediately emits individual `subgift` lines for each recipient)
/// - `PRIVMSG` with `bits=<N>` where `N > 0` -> 1 cheer/bits gift event
pub fn parse_twitch_irc_gift_count(line: &str) -> u64 {
    let trimmed = line.trim();
    let Some(rest) = trimmed.strip_prefix('@') else {
        return 0;
    };
    let Some((tags_str, irc_body)) = rest.split_once(' ') else {
        return 0;
    };

    if irc_body.contains(" USERNOTICE #") {
        for kv in tags_str.split(';') {
            if let Some(val) = kv.strip_prefix("msg-id=")
                && matches!(val, "subgift" | "anonsubgift")
            {
                return 1;
            }
        }
        return 0;
    }

    if irc_body.contains(" PRIVMSG #") {
        for kv in tags_str.split(';') {
            if let Some(val) = kv.strip_prefix("bits=")
                && val.parse::<u64>().is_ok_and(|n| n > 0)
            {
                return 1;
            }
        }
    }

    0
}

pub fn extract_youtube_chat_bootstrap(html: &str) -> Option<(String, String)> {
    let api_key = extract_quoted_value(html, "\"INNERTUBE_API_KEY\":\"")?;
    let live_chat_idx = html.find("\"liveChatRenderer\"")?;
    let tail = &html[live_chat_idx..];
    let cont = extract_quoted_value(tail, "\"continuation\":\"")?;
    Some((api_key, cont))
}

fn extract_quoted_value(haystack: &str, prefix: &str) -> Option<String> {
    let idx = haystack.find(prefix)?;
    let rest = &haystack[idx + prefix.len()..];
    let end = rest.find('"')?;
    let val = &rest[..end];
    if val.is_empty() {
        None
    } else {
        Some(val.to_string())
    }
}

pub fn parse_youtube_live_chat_response(
    val: &serde_json::Value,
) -> (Vec<(String, String)>, Option<String>, u64) {
    let mut msgs = Vec::new();
    let lcc = val.pointer("/continuationContents/liveChatContinuation");
    if let Some(actions) = lcc.and_then(|v| v.get("actions")).and_then(|v| v.as_array()) {
        for act in actions {
            let Some(renderer) = act.pointer("/addChatItemAction/item/liveChatTextMessageRenderer") else {
                continue;
            };
            let author = renderer
                .pointer("/authorName/simpleText")
                .and_then(|v| v.as_str())
                .unwrap_or("anon")
                .to_string();
            let mut text = String::new();
            if let Some(runs) = renderer.pointer("/message/runs").and_then(|v| v.as_array()) {
                for r in runs {
                    if let Some(t) = r.get("text").and_then(|v| v.as_str()) {
                        text.push_str(t);
                    }
                }
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                msgs.push((author, trimmed.to_string()));
            }
        }
    }

    let mut next_cont = None;
    let mut timeout_ms = 5000u64;
    if let Some(conts) = lcc.and_then(|v| v.get("continuations")).and_then(|v| v.as_array()) {
        for c in conts {
            if let Some(inval) = c.get("invalidationContinuationData") {
                if let Some(tok) = inval.get("continuation").and_then(|v| v.as_str()) {
                    next_cont = Some(tok.to_string());
                }
                if let Some(ms) = inval.get("timeoutMs").and_then(|v| v.as_u64()) {
                    timeout_ms = ms;
                }
                break;
            }
            if let Some(timed) = c.get("timedContinuationData") {
                if let Some(tok) = timed.get("continuation").and_then(|v| v.as_str()) {
                    next_cont = Some(tok.to_string());
                }
                if let Some(ms) = timed.get("timeoutMs").and_then(|v| v.as_u64()) {
                    timeout_ms = ms;
                }
                break;
            }
        }
    }

    (msgs, next_cont, timeout_ms.clamp(3000, 10000))
}

impl SupervisorState {
    pub async fn ingest_chat_complaint_message(
        &self,
        channel_id: &str,
        viewer_id: &str,
        text: &str,
    ) {
        let group_info = {
            let map = self.channels.read().await;
            let show = self.show.read().await;
            map.get(channel_id).and_then(|c| {
                show.groups
                    .iter()
                    .find(|g| g.id == c.config.group_id)
                    .map(|g| {
                        (
                            g.id.clone(),
                            g.complaint_keywords.clone(),
                            c.config.name.clone(),
                        )
                    })
            })
        };

        let Some((gid, keywords, ch_name)) = group_info else {
            return;
        };
        if keywords.is_empty() {
            return;
        }

        let alerts_cfg = self.cfg.read().await.alerts.clone();
        let rule = crate::stats::ChatComplaintRule {
            keywords: &keywords,
            window_secs: alerts_cfg.chat_spike_window_secs,
            threshold_unique: alerts_cfg.chat_spike_unique_viewers,
        };
        let spike = self.chat_sensor.lock().await.ingest_message(
            &gid,
            viewer_id,
            text,
            &rule,
            Utc::now(),
        );
        if !spike {
            return;
        }

        {
            let mut cooldown = self.chat_spike_cooldown.lock().await;
            let now = Instant::now();
            if let Some(last) = cooldown.get(&gid)
                && now.duration_since(*last) < Duration::from_secs(60)
            {
                return;
            }
            cooldown.insert(gid.clone(), now);
        }

        let inc = IncidentRecord {
            id: Uuid::new_v4().to_string(),
            channel_id: channel_id.to_string(),
            channel_name: ch_name,
            group_id: gid,
            fault: FaultKind::ChatComplaintSpike,
            severity: AlertState::Warning,
            started_at: Utc::now(),
            resolved_at: Some(Utc::now()),
            duration_secs: 0.0,
            clip_filename: None,
            acknowledged: false,
            detail: format!(
                "{}+ unique viewers reported stream issue in chat",
                alerts_cfg.chat_spike_unique_viewers
            ),
        };
        self.incidents.write().await.insert(0, inc.clone());
        self.db.enqueue(DbCommand::UpsertIncident(inc.clone())).await;
        let _ = self.ws_tx.send(WsBroadcast::Incident { incident: inc });
    }

    pub async fn sync_chat_subscriptions(&self) {
        let desired: Vec<(String, Platform, String)> = self
            .show
            .read()
            .await
            .channels
            .iter()
            .filter(|ch| {
                ch.enabled
                    && !ch.url.starts_with("sim://")
                    && matches!(ch.platform, Platform::Twitch | Platform::Youtube)
            })
            .map(|ch| (ch.id.clone(), ch.platform, ch.url.clone()))
            .collect();

        if let Some(tx) = self.chat_cmd_tx.lock().await.clone() {
            let _ = tx.send(desired).await;
        }
    }

    pub async fn spawn_chat_bridge(self: &Arc<Self>) {
        let (cmd_tx, mut cmd_rx) =
            tokio::sync::mpsc::channel::<Vec<(String, Platform, String)>>(32);
        *self.chat_cmd_tx.lock().await = Some(cmd_tx);
        self.sync_chat_subscriptions().await;

        let state = Arc::clone(self);
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36")
                .build()
                .unwrap_or_default();
            let mut active: HashMap<String, (Platform, String, AbortHandle)> = HashMap::new();
            let mut shutdown_rx = state.subscribe_shutdown();

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                let desired_list = tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    maybe_desired = cmd_rx.recv() => match maybe_desired {
                        Some(list) => list,
                        None => break,
                    },
                };
                let desired: HashMap<String, (Platform, String)> = desired_list
                    .into_iter()
                    .map(|(id, p, u)| (id, (p, u)))
                    .collect();

                let to_remove: Vec<String> = active
                    .iter()
                    .filter(|(id, (p, u, _))| desired.get(*id) != Some(&(*p, u.clone())))
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in to_remove {
                    if let Some((_, _, handle)) = active.remove(&id) {
                        handle.abort();
                    }
                    state.chat_rates.write().await.remove(&id);
                    state.twitch_gifts.write().await.remove(&id);
                }

                for (id, (plat, url)) in desired {
                    if active.contains_key(&id) {
                        continue;
                    }
                    let st = Arc::clone(&state);
                    let cl = client.clone();
                    let ch_id = id.clone();
                    let ch_url = url.clone();
                    let jh = tokio::spawn(async move {
                        match plat {
                            Platform::Twitch => run_twitch_chat_loop(st, ch_id, ch_url).await,
                            Platform::Youtube => run_youtube_chat_loop(st, cl, ch_id, ch_url).await,
                            _ => {}
                        }
                    });
                    active.insert(id, (plat, url, jh.abort_handle()));
                }
            }
            for (_, (_, _, handle)) in active.drain() {
                handle.abort();
            }
        });
    }
}

async fn run_twitch_chat_loop(state: Arc<SupervisorState>, channel_id: String, url: String) {
    let Some(login) = extract_twitch_login(&url) else {
        return;
    };
    let mut tracker = ChatRateTracker::new();
    let nick = format!("justinfan{}", 10000 + (std::process::id() % 89999));

    let mut total_gifts: u64 = 0;

    loop {
        if let Ok((mut ws, _)) = connect_async("wss://irc-ws.chat.twitch.tv:443").await {
            let _ = ws
                .send(Message::Text(
                    "CAP REQ :twitch.tv/tags twitch.tv/commands".into(),
                ))
                .await;
            let _ = ws.send(Message::Text(format!("NICK {nick}").into())).await;
            let _ = ws.send(Message::Text(format!("JOIN #{login}").into())).await;
            // Register channel as connected with initial 0 rate & gift count
            state.chat_rates.write().await.insert(channel_id.clone(), 0.0);
            state
                .twitch_gifts
                .write()
                .await
                .insert(channel_id.clone(), total_gifts as f64);

            let mut decay_tick = tokio::time::interval(Duration::from_secs(10));
            decay_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = decay_tick.tick() => {
                        let rate = tracker.rate_per_min(&channel_id, Instant::now()).unwrap_or(0.0);
                        state.chat_rates.write().await.insert(channel_id.clone(), rate);
                    }
                    maybe_msg = ws.next() => {
                        let Some(Ok(msg)) = maybe_msg else {
                            break;
                        };
                        if let Message::Text(txt) = msg {
                            for line in txt.lines() {
                                if line.starts_with("PING") {
                                    let _ = ws.send(Message::Text("PONG :tmi.twitch.tv".into())).await;
                                    continue;
                                }
                                let gift_delta = parse_twitch_irc_gift_count(line);
                                if gift_delta > 0 {
                                    total_gifts = total_gifts.saturating_add(gift_delta);
                                    state
                                        .twitch_gifts
                                        .write()
                                        .await
                                        .insert(channel_id.clone(), total_gifts as f64);
                                }
                                if let Some((viewer, body)) = parse_twitch_irc_privmsg(line) {
                                    let now = Instant::now();
                                    tracker.record(&channel_id, now);
                                    if let Some(rate) = tracker.rate_per_min(&channel_id, now) {
                                        state.chat_rates.write().await.insert(channel_id.clone(), rate);
                                    }
                                    state.ingest_chat_complaint_message(&channel_id, &viewer, &body).await;
                                }
                            }
                        }
                    }
                }
            }
            state.chat_rates.write().await.insert(channel_id.clone(), 0.0);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn run_youtube_chat_loop(
    state: Arc<SupervisorState>,
    client: reqwest::Client,
    channel_id: String,
    url: String,
) {
    let mut tracker = ChatRateTracker::new();
    let mut bootstrap_misses: u32 = 0;

    loop {
        let Ok(resp) = client
            .get(&url)
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
        else {
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        };
        let content_len = resp.content_length();
        let Ok(html) = resp.text().await else {
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        };
        crate::stats::record_http_rx_bytes(content_len, html.len());
        let Some((api_key, mut continuation)) = extract_youtube_chat_bootstrap(&html) else {
            bootstrap_misses = bootstrap_misses.saturating_add(1);
            let backoff_secs = match bootstrap_misses {
                1 => 15,
                2 => 60,
                _ => 180,
            };
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            continue;
        };
        bootstrap_misses = 0;

        state.chat_rates.write().await.insert(channel_id.clone(), 0.0);
        let endpoint = format!(
            "https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key={api_key}"
        );

        loop {
            let body = serde_json::json!({
                "context": {
                    "client": {
                        "clientName": "WEB",
                        "clientVersion": "2.20250101.00.00"
                    }
                },
                "continuation": continuation
            });

            let Ok(res) = client.post(&endpoint).json(&body).send().await else {
                break;
            };
            let res_len = res.content_length();
            let Ok(raw_bytes) = res.bytes().await else {
                break;
            };
            crate::stats::record_http_rx_bytes(res_len, raw_bytes.len());
            let Ok(val) = serde_json::from_slice::<serde_json::Value>(&raw_bytes) else {
                break;
            };
            let (msgs, next_tok, wait_ms) = parse_youtube_live_chat_response(&val);
            for (viewer, text) in msgs {
                let now = Instant::now();
                tracker.record(&channel_id, now);
                state.ingest_chat_complaint_message(&channel_id, &viewer, &text).await;
            }
            let rate = tracker.rate_per_min(&channel_id, Instant::now()).unwrap_or(0.0);
            state.chat_rates.write().await.insert(channel_id.clone(), rate);
            match next_tok {
                Some(tok) => continuation = tok,
                None => break,
            }
            tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_twitch_irc_privmsg() {
        let line = ":viewer123!viewer123@viewer123.tmi.twitch.tv PRIVMSG #hok_esports :no audio!!";
        assert_eq!(
            parse_twitch_irc_privmsg(line),
            Some(("viewer123".to_string(), "no audio!!".to_string()))
        );
        let tagged = "@badge-info=;badges=subscriber/3;bits=100;color=#FF0000 :viewer99!viewer99@viewer99.tmi.twitch.tv PRIVMSG #hok_esports :Cheer100 nice play!";
        assert_eq!(
            parse_twitch_irc_privmsg(tagged),
            Some(("viewer99".to_string(), "Cheer100 nice play!".to_string()))
        );
        assert_eq!(parse_twitch_irc_privmsg("PING :tmi.twitch.tv"), None);
    }

    #[test]
    fn test_parse_twitch_irc_gift_count() {
        let subgift = "@badge-info=;msg-id=subgift;msg-param-months=1 :tmi.twitch.tv USERNOTICE #hok_esports";
        assert_eq!(parse_twitch_irc_gift_count(subgift), 1);

        let anon = "@msg-id=anonsubgift;room-id=123 :tmi.twitch.tv USERNOTICE #hok_esports";
        assert_eq!(parse_twitch_irc_gift_count(anon), 1);

        // Mystery gift header is ignored because Twitch immediately sends individual subgift lines.
        let mystery = "@msg-id=submysterygift;msg-param-mass-gift-count=5 :tmi.twitch.tv USERNOTICE #hok_esports";
        assert_eq!(parse_twitch_irc_gift_count(mystery), 0);

        let cheer = "@bits=500;display-name=Fan :fan!fan@fan.tmi.twitch.tv PRIVMSG #hok_esports :Cheer500";
        assert_eq!(parse_twitch_irc_gift_count(cheer), 1);

        let normal = "@display-name=Fan :fan!fan@fan.tmi.twitch.tv PRIVMSG #hok_esports :hello";
        assert_eq!(parse_twitch_irc_gift_count(normal), 0);
    }

    #[test]
    fn test_extract_youtube_chat_bootstrap_and_parse_response() {
        let html = r#"<script>ytcfg.set({"INNERTUBE_API_KEY":"AIzaTestKey123"});</script><script>{"liveChatRenderer":{"continuations":[{"reloadContinuationData":{"continuation":"0ofMy_ContToken"}}]}};</script>"#;
        assert_eq!(
            extract_youtube_chat_bootstrap(html),
            Some(("AIzaTestKey123".to_string(), "0ofMy_ContToken".to_string()))
        );

        let payload = serde_json::json!({
            "continuationContents": {
                "liveChatContinuation": {
                    "actions": [
                        {
                            "addChatItemAction": {
                                "item": {
                                    "liveChatTextMessageRenderer": {
                                        "authorName": { "simpleText": "CasterFan" },
                                        "message": { "runs": [{ "text": "suara ilang " }, { "text": "min" }] }
                                    }
                                }
                            }
                        }
                    ],
                    "continuations": [
                        {
                            "timedContinuationData": {
                                "continuation": "NextTok999",
                                "timeoutMs": 4500
                            }
                        }
                    ]
                }
            }
        });
        let (msgs, next_tok, wait_ms) = parse_youtube_live_chat_response(&payload);
        assert_eq!(msgs, vec![("CasterFan".to_string(), "suara ilang min".to_string())]);
        assert_eq!(next_tok.as_deref(), Some("NextTok999"));
        assert_eq!(wait_ms, 4500);
    }

    #[test]
    fn test_chat_rate_tracker_prunes_60s_window() {
        let mut tracker = ChatRateTracker::new();
        let t0 = Instant::now();
        tracker.record("ch-1", t0);
        tracker.record("ch-1", t0 + Duration::from_secs(10));
        assert_eq!(tracker.rate_per_min("ch-1", t0 + Duration::from_secs(30)), Some(2.0));
        assert_eq!(tracker.rate_per_min("ch-1", t0 + Duration::from_secs(65)), Some(1.0));
    }
}

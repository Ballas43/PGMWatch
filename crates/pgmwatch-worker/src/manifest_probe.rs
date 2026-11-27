use std::time::{Duration, Instant};

pub struct ManifestProbe {
    client: reqwest::Client,
    last_media_seq: Option<u64>,
    unchanged_polls: u32,
    consecutive_errors: u32,
    consecutive_slow_downloads: u32,
}

#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    pub ad_break: bool,
    pub stalled: bool,
    pub cdn_starvation: bool,
    pub cdn_ratio: f32,
    pub token_expired: bool,
}

impl ManifestProbe {
    pub fn new(bind_ip: Option<&str>) -> anyhow::Result<Self> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .user_agent("PGMWatch/0.1");
        if let Some(ip) = bind_ip
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        {
            builder = builder.local_address(ip);
        }
        Ok(Self {
            client: builder.build()?,
            last_media_seq: None,
            unchanged_polls: 0,
            consecutive_errors: 0,
            consecutive_slow_downloads: 0,
        })
    }

    pub async fn probe(&mut self, url: &str, headers: &[(String, String)]) -> ProbeOutcome {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("sim://")
            || lower.starts_with("rtmp://")
            || lower.starts_with("rtmps://")
            || lower.starts_with("srt://")
        {
            return ProbeOutcome {
                ad_break: lower.contains("ad_break"),
                stalled: false,
                cdn_starvation: false,
                cdn_ratio: 0.18,
                token_expired: false,
            };
        }

        let is_playlist_url = lower.contains(".m3u8")
            || lower.contains(".mpd")
            || lower.contains("playlist")
            || lower.contains("manifest");

        // Continuous HTTP media streams (e.g. TikTok/Bigo live HTTP-FLV) have no text manifest.
        // Live FLV CDNs ignore Range headers and push multi-megabyte fast-start GOP bursts on every
        // GET, which would burn tens of Mbps every 2s across workers while FFmpeg already monitors
        // the live socket directly.
        if !is_playlist_url {
            return ProbeOutcome {
                ad_break: false,
                stalled: false,
                cdn_starvation: false,
                cdn_ratio: 0.18,
                token_expired: false,
            };
        }

        let started = Instant::now();
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }

        match req.send().await {
            Ok(resp) => {
                self.consecutive_errors = 0;
                let status = resp.status();
                if status.as_u16() == 401 || status.as_u16() == 403 || status.as_u16() == 410 {
                    return ProbeOutcome {
                        ad_break: false,
                        stalled: false,
                        cdn_starvation: false,
                        cdn_ratio: 1.0,
                        token_expired: true,
                    };
                }
                let ct = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let is_text_manifest = is_playlist_url
                    || ct.contains("mpegurl")
                    || ct.contains("dash+xml")
                    || ct.starts_with("text/");
                let elapsed = started.elapsed().as_secs_f32();
                if !is_text_manifest {
                    drop(resp);
                    return ProbeOutcome {
                        ad_break: false,
                        stalled: false,
                        cdn_starvation: false,
                        cdn_ratio: elapsed.clamp(0.05, 0.9),
                        token_expired: false,
                    };
                }

                let body = resp.text().await.unwrap_or_default();
                let target_dur = parse_hls_target_duration(&body).unwrap_or(2.0);
                let cdn_ratio = elapsed / target_dur.max(0.5);
                if cdn_ratio > 0.85 {
                    self.consecutive_slow_downloads += 1;
                } else {
                    self.consecutive_slow_downloads = 0;
                }

                let ad_break = is_hls_ad_break(&body) || is_dash_ad_break(&body);

                let seq = parse_hls_media_sequence(&body);
                if let (Some(prev), Some(curr)) = (self.last_media_seq, seq) {
                    if prev == curr {
                        self.unchanged_polls += 1;
                    } else {
                        self.unchanged_polls = 0;
                    }
                }
                if seq.is_some() {
                    self.last_media_seq = seq;
                }

                ProbeOutcome {
                    ad_break,
                    stalled: self.unchanged_polls >= 3,
                    cdn_starvation: self.consecutive_slow_downloads >= 3,
                    cdn_ratio,
                    token_expired: false,
                }
            }
            Err(_) => {
                self.consecutive_errors += 1;
                ProbeOutcome {
                    ad_break: false,
                    stalled: self.consecutive_errors >= 3,
                    cdn_starvation: false,
                    cdn_ratio: 1.0,
                    token_expired: false,
                }
            }
        }
    }
}

fn parse_hls_target_duration(body: &str) -> Option<f32> {
    body.lines()
        .filter_map(|line| line.strip_prefix("#EXT-X-TARGETDURATION:"))
        .find_map(|rest| rest.trim().parse::<f32>().ok())
}

fn parse_hls_media_sequence(body: &str) -> Option<u64> {
    body.lines()
        .filter_map(|line| line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:"))
        .find_map(|rest| rest.trim().parse::<u64>().ok())
}

pub fn is_hls_ad_break(body: &str) -> bool {
    let mut cue_out_active = false;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if !line.starts_with('#') {
            continue;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("#EXT-X-DATERANGE")
            && (upper.contains("TWITCH-STITCHED-AD")
                || upper.contains("SCTE35-OUT")
                || upper.contains("X-TV-TWITCH-AD")
                || upper.contains("AD-BREAK"))
        {
            return true;
        } else if upper.starts_with("#EXT-X-CUE-OUT") {
            cue_out_active = true;
        } else if upper.starts_with("#EXT-X-CUE-IN") {
            cue_out_active = false;
        }
    }
    cue_out_active
}

pub fn is_dash_ad_break(xml: &str) -> bool {
    let lower = xml.to_ascii_lowercase();
    if !lower.contains("<mpd") && !lower.contains("<period") && !lower.contains("<eventstream") {
        return false;
    }
    if lower.contains("<eventstream")
        && (lower.contains("urn:scte:scte35")
            || lower.contains("scte35")
            || (lower.contains("urn:mpeg:dash:event:2012")
                && (lower.contains("splice_insert")
                    || lower.contains("ad_break")
                    || lower.contains("cue-out")
                    || lower.contains("adbreak"))))
    {
        return true;
    }
    for seg in lower.split("<period").skip(1) {
        let tag_head = seg.split('>').next().unwrap_or("");
        if let Some(pos) = tag_head.find("id=") {
            let after = &tag_head[pos + 3..];
            let id_val = after
                .trim_start_matches(['"', '\''])
                .split(['"', '\'', ' '])
                .next()
                .unwrap_or("");
            if id_val.contains("ad-")
                || id_val.contains("_ad_")
                || id_val.contains("adbreak")
                || id_val.contains("preroll")
                || id_val.contains("midroll")
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{is_dash_ad_break, is_hls_ad_break, ManifestProbe};

    #[test]
    fn test_dash_ad_break_detection() {
        let scte35_mpd = r#"<MPD><Period id="main"><EventStream schemeIdUri="urn:scte:scte35:2013:xml"><Event id="1"/></EventStream></Period></MPD>"#;
        assert!(is_dash_ad_break(scte35_mpd));

        let ad_period_mpd = r#"<MPD><Period id="midroll-ad-1"></Period></MPD>"#;
        assert!(is_dash_ad_break(ad_period_mpd));

        let clean_mpd = r#"<MPD><Period id="p1"><AdaptationSet><Representation id="v1"/></AdaptationSet></Period></MPD>"#;
        assert!(!is_dash_ad_break(clean_mpd));
    }

    #[test]
    fn test_hls_ad_break_detection() {
        let live_twitch = "\
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:6
#EXT-X-DATERANGE:ID=\"playlist-session-1\",CLASS=\"twitch-session\",START-DATE=\"2026-11-21T15:31:58Z\"
#EXT-X-DATERANGE:ID=\"source-1\",CLASS=\"twitch-stream-source\",START-DATE=\"2026-11-21T15:29:29Z\"
#EXT-X-DISCONTINUITY
#EXTINF:2.000,live
https://cdn.example.net/v1/segment/RELlXBADQogSCTE35AD_test.ts
";
        assert!(!is_hls_ad_break(live_twitch));

        let real_twitch_ad = "\
#EXTM3U
#EXT-X-DATERANGE:ID=\"stitched-ad-1\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-11-21T15:35:00Z\"
#EXTINF:2.000,live
https://cdn.example.net/ad.ts
";
        assert!(is_hls_ad_break(real_twitch_ad));

        let active_cue = "#EXTM3U\n#EXT-X-CUE-OUT:30\n#EXTINF:2.000,\nseg.ts\n";
        assert!(is_hls_ad_break(active_cue));

        let finished_cue = "#EXTM3U\n#EXT-X-CUE-OUT:30\n#EXTINF:2.000,\nseg1.ts\n#EXT-X-CUE-IN\n#EXTINF:2.000,\nseg2.ts\n";
        assert!(!is_hls_ad_break(finished_cue));
    }

    #[tokio::test]
    async fn test_manifest_probe_skips_rtmp_and_srt() {
        let mut probe = ManifestProbe::new(None).unwrap();
        for url in [
            "rtmp://127.0.0.1:1935/live/test",
            "srt://127.0.0.1:9000?mode=caller",
            "http://127.0.0.1:1/game/stream-12345_ld5.flv?expire=999",
        ] {
            for _ in 0..4 {
                let res = probe.probe(url, &[]).await;
                assert!(!res.stalled, "expected continuous stream probe never stalled for {url}");
                assert!(!res.token_expired);
            }
        }
    }
}

use super::*;
use super::mpd_parser::{format_hls_v7_playlist, parse_dash_representation_to_segments};

    const SAMPLE_FB_MPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic">
  <Period id="0">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="dash-lp-pst-v" bandwidth="2500000" width="1280" height="720">
        <SegmentTemplate timescale="1000" initialization="../dash-lp-pst-v/init.mp4?a=1&amp;b=2" media="../dash-lp-pst-v/$Time$.m4v?a=1&amp;b=2">
          <SegmentTimeline>
            <S t="10000" d="2000" r="2"/>
            <S d="2000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="dash-lp-md-a" bandwidth="128000">
        <SegmentTemplate timescale="1000" initialization="../dash-lp-md-a/init.mp4?tok=x&amp;y=1" media="../dash-lp-md-a/$Time$.m4a?tok=x&amp;y=1">
          <SegmentTimeline>
            <S t="10000" d="2000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn parses_facebook_dynamic_mpd_relative_urls_and_entities() {
        let mpd_url = "https://video.xx.fbcdn.net/v/t66/live-dash/1161684949951264.mpd";
        let parsed = parse_dash_representation_to_segments(SAMPLE_FB_MPD, mpd_url, "dash-lp-pst-v-2")
            .expect("should match dash-lp-pst-v from yt-dlp format_id dash-lp-pst-v-2");

        assert_eq!(
            parsed.init_url,
            "https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/init.mp4?a=1&b=2"
        );
        assert_eq!(parsed.target_duration, 2);
        assert_eq!(parsed.segments.len(), 4);
        assert_eq!(parsed.segments[0].start_tick, 10000);
        assert_eq!(
            parsed.segments[0].url,
            "https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/10000.m4v?a=1&b=2"
        );
        assert_eq!(parsed.segments[2].start_tick, 14000);
        assert_eq!(parsed.segments[3].start_tick, 16000);

        let m3u8 = format_hls_v7_playlist(&parsed, 7, false);
        assert!(m3u8.contains("#EXT-X-VERSION:7\n"));
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:7\n"));
        assert!(m3u8.contains("#EXT-X-MAP:URI=\"https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/init.mp4?a=1&b=2\"\n"));
        assert!(m3u8.contains("#EXTINF:2.000,\n"));
    }

    #[test]
    fn parses_audio_representation_exact_and_resolves_base_url() {
        let mpd = r#"<MPD>
  <BaseURL>https://cdn.example.com/live/sub/</BaseURL>
  <Period>
    <AdaptationSet>
      <Representation id="dash-lp-md-a">
        <SegmentTemplate timescale="48000" initialization="init.mp4" media="$Time$.m4a">
          <SegmentTimeline>
            <S t="480000" d="96000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let parsed = parse_dash_representation_to_segments(
            mpd,
            "https://other.example.com/manifest.mpd",
            "dash-lp-md-a",
        )
        .expect("exact rep id should match");
        assert_eq!(parsed.init_url, "https://cdn.example.com/live/sub/init.mp4");
        assert_eq!(parsed.segments[0].url, "https://cdn.example.com/live/sub/480000.m4a");
        assert!((parsed.segments[0].duration_secs - 2.0).abs() < 1e-6);
    }

    #[test]
    fn advances_media_sequence_monotonically_across_sliding_windows() {
        let mut entry = RelayEntry {
            kind: RelayKind::DashMpd {
                mpd_url: "https://example.com/live.mpd".into(),
                format_id: "dash-lp-pst-v".into(),
                last_times: Vec::new(),
                media_sequence: 0,
            },
            headers: vec![],
            last_accessed: Instant::now(),
        };
        assert_eq!(advance_media_sequence(&mut entry, &[100, 200, 300, 400]), 0);
        // Same window (cache hit) keeps sequence 0
        assert_eq!(advance_media_sequence(&mut entry, &[100, 200, 300, 400]), 0);
        // Window slid forward by 2 segments
        assert_eq!(advance_media_sequence(&mut entry, &[300, 400, 500, 600]), 2);
        // Disjoint forward jump advances cleanly
        assert_eq!(advance_media_sequence(&mut entry, &[900, 1000, 1100]), 6);
    }

    #[test]
    fn identifies_youtube_dvr_hls_manifests_for_gzip_proxy() {
        assert!(should_proxy_hls_manifest(
            "https://manifest.googlevideo.com/api/manifest/hls_playlist/playlist_type/DVR/playlist/index.m3u8"
        ));
        assert!(should_proxy_hls_manifest(
            "https://cdn.example.com/live/manifest_duration/3600/index.m3u8"
        ));
        assert!(!should_proxy_hls_manifest(
            "http://127.0.0.1:8787/api/internal/dash-hls/abc123.m3u8"
        ));
        assert!(!should_proxy_hls_manifest(
            "https://pull-flv.tiktokcdn.com/stage/stream-123.flv"
        ));
    }

    #[test]
    fn trims_large_live_dvr_hls_playlist_to_live_edge() {
        let mut raw = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:2400\n#EXT-X-DISCONTINUITY-SEQUENCE:1\n",
        );
        for i in 0..25 {
            if i == 5 {
                raw.push_str("#EXT-X-DISCONTINUITY\n");
            }
            raw.push_str(&format!("#EXTINF:5.005,\nseg_{i}.ts\n"));
        }
        let trimmed = trim_hls_dvr_playlist(
            "https://manifest.googlevideo.com/api/manifest/hls_playlist/index.m3u8",
            &raw,
            12,
        );
        assert!(trimmed.contains("#EXT-X-MEDIA-SEQUENCE:2413\n"));
        assert!(trimmed.contains("#EXT-X-DISCONTINUITY-SEQUENCE:2\n"));
        assert!(!trimmed.contains("seg_12.ts"));
        assert!(trimmed.contains("https://manifest.googlevideo.com/api/manifest/hls_playlist/seg_13.ts"));
        assert!(trimmed.contains("https://manifest.googlevideo.com/api/manifest/hls_playlist/seg_24.ts"));
        assert_eq!(trimmed.matches("#EXTINF:").count(), 12);
    }

    #[test]
    fn leaves_short_or_vod_hls_playlists_unchanged() {
        let vod = "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:5.0,\nseg0.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            trim_hls_dvr_playlist("https://example.com/vod.m3u8", vod, 12),
            "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:5.0,\nhttps://example.com/seg0.ts\n#EXT-X-ENDLIST\n"
        );
    }

    #[test]
    fn emits_hls_cue_out_when_dash_mpd_contains_scte35_ad_event() {
        let mpd = r#"<MPD>
  <BaseURL>https://cdn.example.com/live/</BaseURL>
  <Period id="main">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml"><Event id="10"/></EventStream>
    <AdaptationSet>
      <Representation id="v1">
        <SegmentTemplate timescale="1000" initialization="init.mp4" media="$Time$.m4v">
          <SegmentTimeline><S t="1000" d="2000"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        assert!(is_dash_mpd_ad_break(mpd));
        let parsed = parse_dash_representation_to_segments(mpd, "https://cdn.example.com/live.mpd", "v1").unwrap();
        let m3u8 = format_hls_v7_playlist(&parsed, 1, true);
        assert!(m3u8.contains("#EXT-X-CUE-OUT:30\n"));
        let clean = format_hls_v7_playlist(&parsed, 1, false);
        assert!(!clean.contains("#EXT-X-CUE-OUT"));
    }

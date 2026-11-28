use super::*;
use super::extractors::{
    build_streamlink_args, build_yt_dlp_args, build_yt_extractor_args, classify_ytdlp_stderr,
    select_streamlink_quality, select_ytdlp_format, select_ytdlp_formats, yt_format_for,
};
use super::live_check::{
    check_ytdlp_live_status, classify_hls_manifest_vod, first_hls_variant_url,
    is_always_live_transport_url, is_static_vod_media_url,
};

    #[test]
    fn test_classify_ytdlp_stderr_facebook_event_offline() {
        assert!(matches!(
            classify_ytdlp_stderr(
                "https://www.facebook.com/events/29203231515964504/",
                "ERROR: [facebook] 29203231515964504: No video found. Check if the video is private. Use --cookies-from-browser or --cookies for authentication.".into()
            ),
            ResolveError::Offline(_)
        ));
        assert!(matches!(
            classify_ytdlp_stderr(
                "https://www.youtube.com/watch?v=abc",
                "ERROR: [youtube] abc: Sign in to confirm you're not a bot. Use --cookies-from-browser".into()
            ),
            ResolveError::LoginGated(_)
        ));
        let sample_ssr = r#"{"videoDeliveryLegacyFields":null,"videoDeliveryResponseFragment":{"videoDeliveryResponseResult":{"dash_manifest_urls":[{"manifest_url":"https:\/\/scontent.fpen3-1.fna.fbcdn.net\/live-dash\/1168362549283504.mpd?_nc_zt=28\u0026ccb=2-4"}]}}}"#;
        assert_eq!(
            extract_facebook_dash_manifest_url(
                sample_ssr,
                "https://www.facebook.com/page/videos/1168362549283504"
            )
            .as_deref(),
            Some("https://scontent.fpen3-1.fna.fbcdn.net/live-dash/1168362549283504.mpd?_nc_zt=28&ccb=2-4")
        );
        let sample_legacy = r#"{"videoDeliveryLegacyFields":{"dash_manifest_url":"https:\/\/video.fkul8-5.fna.fbcdn.net\/live-dash\/1169385292514563.mpd?_nc_zt=28&amp;ccb=2-4"}}"#;
        assert_eq!(
            extract_facebook_dash_manifest_url(
                sample_legacy,
                "https://www.facebook.com/HonorofKingsGlobal/videos/1422178356342896"
            )
            .as_deref(),
            Some("https://video.fkul8-5.fna.fbcdn.net/live-dash/1169385292514563.mpd?_nc_zt=28&ccb=2-4")
        );
    }

    #[tokio::test]
    async fn test_url_resolver_cache_and_invalidate() {
        let guard = Arc::new(WindowsJobGuard::new(512).expect("job guard"));
        let dash_relay = Arc::new(DashRelayState::new(8787));
        let resolver = UrlResolver::new(
            ExtractionConfig {
                auth_mode: "anonymous".into(),
                max_concurrent_extractors: 2,
                enable_bgutil_pot: false,
                bgutil_pot_port: 4416,
                msedge_path: String::new(),
            },
            PathBuf::from("."),
            PathBuf::from("."),
            guard,
            dash_relay,
        );
        let url = "https://youtube.com/watch?v=test_cache";
        resolver.cache.lock().await.insert(
            (url.to_string(), FeedTier::Master, true),
            (
                ResolvedMedia {
                    stream_url: "https://manifest.example/cached.m3u8".into(),
                    audio_url: None,
                    headers: vec![],
                    via: "yt-dlp".into(),
                },
                Instant::now(),
            ),
        );
        let hit = resolver.resolve(url, Platform::Youtube, FeedTier::Master, true).await.unwrap();
        assert_eq!(hit.stream_url, "https://manifest.example/cached.m3u8");
        resolver.invalidate_url(url).await;
        assert!(resolver.cache.lock().await.is_empty());
    }

    #[test]
    fn test_build_yt_extractor_args() {
        assert_eq!(build_yt_extractor_args(None, false, 4416), None);
        assert_eq!(
            build_yt_extractor_args(Some("youtube:player_client=tv_simply"), false, 4416).as_deref(),
            Some("youtube:player_client=tv_simply")
        );
        assert_eq!(
            build_yt_extractor_args(None, true, 4416).as_deref(),
            Some("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416")
        );
        assert_eq!(
            build_yt_extractor_args(Some("youtube:player_client=tv_simply"), true, 4499).as_deref(),
            Some("youtube:player_client=tv_simply;youtubepot-bgutilhttp:base_url=http://127.0.0.1:4499")
        );
    }

    #[test]
    fn test_select_ytdlp_formats_pairs_null_acodec_hls_audio() {
        let paired = vec![
            serde_json::json!({
                "format_id": "312",
                "vcodec": "avc1.4D402A",
                "acodec": "none",
                "audio_ext": "none",
                "resolution": "1920x1080"
            }),
            serde_json::json!({
                "format_id": "234",
                "vcodec": "none",
                "acodec": null,
                "audio_ext": "mp4",
                "resolution": "audio only"
            }),
        ];
        let (vid, aud) = select_ytdlp_formats(&paired);
        assert_eq!(vid.and_then(|v| v["format_id"].as_str()), Some("312"));
        assert_eq!(aud.and_then(|a| a["format_id"].as_str()), Some("234"));

        // Single video-only track with null acodec must not self-pair as audio.
        let single_vid = vec![serde_json::json!({
            "format_id": "312",
            "vcodec": "avc1.4D402A",
            "acodec": null,
            "audio_ext": "none",
            "resolution": "1920x1080"
        })];
        let (vid2, aud2) = select_ytdlp_formats(&single_vid);
        assert_eq!(vid2.and_then(|v| v["format_id"].as_str()), Some("312"));
        assert!(aud2.is_none());
    }

    #[test]
    fn test_direct_stream_urls_and_outbound_bind_args() {
        assert!(is_direct_stream_url("rtmp://ingest.local/app/key"));
        assert!(is_direct_stream_url("rtmps://ingest.local/app/key"));
        assert!(is_direct_stream_url("srt://192.168.10.20:9000?mode=caller"));
        assert!(is_direct_stream_url("https://cdn.example.com/live/master.m3u8?token=abc"));
        assert!(!is_direct_stream_url("https://www.youtube.com/watch?v=1gilXpZSyKo"));

        let sl_args = build_streamlink_args("https://twitch.tv/test", "best", Some("192.168.10.50"));
        assert!(sl_args.windows(2).any(|w| w == ["--interface", "192.168.10.50"]));

        let yt_args = build_yt_dlp_args(
            "https://youtube.com/watch?v=abc",
            "best",
            None,
            None,
            None,
            Some("192.168.10.50"),
        );
        assert!(yt_args.windows(2).any(|w| w == ["--source-address", "192.168.10.50"]));
    }

    #[test]
    fn test_live_vs_vod_detection_helpers() {
        assert_eq!(
            check_ytdlp_live_status(&serde_json::json!({ "live_status": "is_live", "is_live": true })),
            Ok(true)
        );
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "was_live" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "not_live" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "is_upcoming" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "is_live": false })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "was_live": true })).is_err());
        assert_eq!(check_ytdlp_live_status(&serde_json::json!({})), Ok(false));

        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXTINF:2.0,\nseg0.ts\n#EXT-X-ENDLIST\n").is_some());
        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:2.0,\nseg0.ts\n").is_some());
        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:2.0,\nseg100.ts\n").is_none());
        assert_eq!(
            first_hls_variant_url(
                "https://cdn.example.com/live/master.m3u8",
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n720p/index.m3u8\n"
            ).as_deref(),
            Some("https://cdn.example.com/live/720p/index.m3u8")
        );

        assert!(classify_dash_manifest_vod("<MPD type=\"static\" mediaPresentationDuration=\"PT30M\">").is_some());
        assert!(classify_dash_manifest_vod("<MPD type=\"dynamic\" minimumUpdatePeriod=\"PT2S\">").is_none());

        assert!(is_static_vod_media_url("https://cdn.example.com/video/replay.mp4?token=1"));
        assert!(!is_static_vod_media_url("https://cdn.example.com/live/stream.flv"));
        assert!(!is_static_vod_media_url("https://cdn.example.com/live/master.m3u8"));
        assert!(is_always_live_transport_url("sim://channel_1"));
        assert!(is_always_live_transport_url("rtmp://ingest.local/app/key"));
    }

    #[test]
    fn test_light_and_simulcast_formats_never_fallback_to_1080p_best() {
        let yt_light = select_ytdlp_format(Platform::Youtube, FeedTier::Light);
        assert!(!yt_light.contains("best[height>=360]"));
        assert!(yt_light.contains("worst[height>=360]"));
        let tv_light = yt_format_for(yt_light, true);
        assert_ne!(tv_light, "bestaudio/best");
        assert!(tv_light.contains("worst[height>=360]"));

        let yt_sim = yt_format_for(select_ytdlp_format(Platform::Youtube, FeedTier::Simulcast), false);
        assert!(yt_sim.contains("height<=480"));
        assert!(!yt_sim.contains("height<=1080"));
        assert!(!yt_sim.ends_with("/best"));

        let fb_sim = select_ytdlp_format(Platform::Facebook, FeedTier::Simulcast);
        assert!(fb_sim.contains("height<=480"));
        assert!(!fb_sim.ends_with("/best"));

        let twitch_sim = select_ytdlp_format(Platform::Twitch, FeedTier::Simulcast);
        assert!(twitch_sim.contains("height<=480"));
        assert!(!twitch_sim.ends_with("/best"));

        assert_eq!(
            select_streamlink_quality(Platform::Tiktok, FeedTier::Light),
            "h264_ao,h265_ao,audio_only,h265_ld,h264_ld,ld,sd,360p,worst"
        );
        let tt_sim = select_streamlink_quality(Platform::Tiktok, FeedTier::Simulcast);
        assert!(tt_sim.contains("h265_sd"));
        assert!(!tt_sim.ends_with(",best"));
        assert_eq!(
            select_streamlink_quality(Platform::Twitch, FeedTier::Light),
            "audio_only,160p,360p,480p,worst"
        );
    }

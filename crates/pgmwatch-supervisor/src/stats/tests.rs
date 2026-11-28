use super::*;
use super::facebook::parse_facebook_html_stats;
use super::tiktok::parse_tiktok_public_ccv;
use super::youtube_twitch::parse_youtube_html_likes;

    #[test]
    fn extracts_twitch_login_with_live_suffix_and_queries() {
        assert_eq!(
            extract_twitch_login("https://www.twitch.tv/hok_esports/live").as_deref(),
            Some("hok_esports")
        );
        assert_eq!(
            extract_twitch_login("https://twitch.tv/ESL_Dota2?referrer=raid").as_deref(),
            Some("esl_dota2")
        );
        assert_eq!(
            extract_twitch_login("https://www.twitch.tv/popout/hok_esports/chat").as_deref(),
            Some("hok_esports")
        );
        assert_eq!(extract_twitch_login("https://www.twitch.tv/directory/game/Honor"), None);
    }

    #[test]
    fn parses_facebook_public_viewer_and_play_count() {
        let html = r#"<script>{"live_viewer_count":null,"live_viewer_count_read_only":2,"play_count":154,"reaction_count":33,"share_count":7,"comment_count":19}</script>"#;
        let parsed = parse_facebook_html_stats(
            "ch-fb-main",
            "https://www.facebook.com/events/1702525898135304/",
            html,
        )
        .expect("FB ccv should parse");
        assert_eq!(parsed.ccv.status, MetricStatus::Value);
        assert_eq!(parsed.ccv.value, Some(2.0));
        assert_eq!(parsed.total_views.status, MetricStatus::Value);
        assert_eq!(parsed.total_views.value, Some(154.0));
        assert_eq!(parsed.likes.value, Some(33.0));
        assert_eq!(parsed.shares.value, Some(7.0));
        assert_eq!(parsed.chat_rate_per_min.status, MetricStatus::Na);
        assert_eq!(parsed.extras.get("comment_count"), Some(&serde_json::json!(19)));

        let escaped = r#"{\"viewer_count\": 42, \"video_view_count\": 901, \"reaction_count\": {\"count\": 128}, \"share_count\": {\"count\": 14}}"#;
        let parsed_escaped = parse_facebook_html_stats(
            "ch-fb-2",
            "https://www.facebook.com/events/1702525898135304/",
            escaped,
        )
        .expect("escaped FB JSON should parse");
        assert_eq!(parsed_escaped.ccv.value, Some(42.0));
        assert_eq!(parsed_escaped.total_views.value, Some(901.0));
        assert_eq!(parsed_escaped.likes.value, Some(128.0));
        assert_eq!(parsed_escaped.shares.value, Some(14.0));

        // Sidebar recommended video belonging to a different video ID must NOT be attributed to target /videos/<id>
        let sidebar_only = r#"{"video":{"is_live_streaming":true,"live_viewer_count_read_only":3,"id":"1013575908466414"}}"#;
        assert!(
            parse_facebook_html_stats(
                "ch-fb-my",
                "https://www.facebook.com/hokmyofficial/videos/1880057129693239",
                sidebar_only,
            )
            .is_none()
        );

        // Comet /videos/ pages expose live viewers under camelCase `liveViewerCount` near the target video ID
        let comet_video = r#"{"video":{"id":"1880057129693239","liveViewerCount":157,"reaction_count":{"count":12}}}"#;
        let parsed_comet = parse_facebook_html_stats(
            "ch-fb-my",
            "https://www.facebook.com/hokmyofficial/videos/1880057129693239",
            comet_video,
        )
        .expect("Comet liveViewerCount should parse");
        assert_eq!(parsed_comet.ccv.value, Some(157.0));
        assert_eq!(parsed_comet.likes.value, Some(12.0));
    }

    #[test]
    fn parses_youtube_accessibility_likes_and_zero_likes() {
        let with_likes = r#"{"likeCountEntity":{"key":"unset_like_count_entity_key"},"accessibilityText":"like this video along with 1,272 other people"}"#;
        assert_eq!(parse_youtube_html_likes(with_likes), Some(1272.0));

        let zero_likes = r#"{"likeCountEntity":{"key":"unset_like_count_entity_key"}}"#;
        assert_eq!(parse_youtube_html_likes(zero_likes), Some(0.0));

        assert_eq!(parse_youtube_html_likes("<html>no player</html>"), None);
    }

    #[test]
    fn extracts_tiktok_username_from_live_and_profile_urls() {
        assert_eq!(
            extract_tiktok_username("https://www.tiktok.com/@hokglobal/live").as_deref(),
            Some("hokglobal")
        );
        assert_eq!(
            extract_tiktok_username("https://tiktok.com/@my_channel?lang=en").as_deref(),
            Some("my_channel")
        );
        assert_eq!(
            extract_tiktok_username("https://www.tiktok.com/hokglobal/live").as_deref(),
            Some("hokglobal")
        );
        assert_eq!(extract_tiktok_username("https://www.tiktok.com/explore"), None);
        assert_eq!(
            parse_tiktok_public_ccv(r#"{"liveRoom":{"liveRoomStats":{"userCount":147}}}"#),
            Some(147.0)
        );
        assert_eq!(
            parse_tiktok_public_ccv(r#"{"roomInfo":{"user_count":72}}"#),
            Some(72.0)
        );
        assert_eq!(parse_tiktok_public_ccv("<html>offline</html>"), None);
    }

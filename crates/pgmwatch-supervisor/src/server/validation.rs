use pgmwatch_core::models::{FeedTier, ShowPreset};

pub(super) fn tier_cost(tier: FeedTier) -> u8 {
    match tier {
        FeedTier::Light => 0,
        FeedTier::Simulcast => 1,
        FeedTier::Master => 2,
    }
}

pub(super) fn validate_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 80
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(super) fn validate_safe_clip_filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name.ends_with(".mp4")
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub(super) fn validate_stream_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    (lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("rtmp://")
        || lower.starts_with("rtmps://")
        || lower.starts_with("srt://")
        || lower.starts_with("sim://"))
        && !trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
}

pub(super) fn validate_show_preset(preset: &ShowPreset) -> Result<(), String> {
    if !validate_safe_id(&preset.show_id) {
        return Err("Invalid show_id (allowed: ASCII alphanumeric, '-', '_')".into());
    }
    if preset.title.trim().is_empty() || preset.title.len() > 160 {
        return Err("Show title must be 1..160 characters".into());
    }
    if preset.groups.is_empty() {
        return Err("At least one language group is required".into());
    }
    let mut group_ids = std::collections::HashSet::with_capacity(preset.groups.len());
    for g in &preset.groups {
        if !validate_safe_id(&g.id) {
            return Err(format!("Invalid group id: {}", g.id));
        }
        if !group_ids.insert(g.id.as_str()) {
            return Err(format!("Duplicate language group id: {}", g.id));
        }
        if g.name.trim().is_empty() || g.name.len() > 80 {
            return Err(format!("Invalid language group name for {}", g.id));
        }
        if g.language_code.trim().is_empty() || g.language_code.len() > 16 {
            return Err(format!("Invalid language code for {}", g.id));
        }
    }
    let mut channel_ids = std::collections::HashSet::with_capacity(preset.channels.len());
    for ch in &preset.channels {
        if !validate_safe_id(&ch.id) {
            return Err(format!("Invalid channel id: {}", ch.id));
        }
        if !channel_ids.insert(ch.id.as_str()) {
            return Err(format!("Duplicate channel id: {}", ch.id));
        }
        if ch.name.trim().is_empty() || ch.name.len() > 120 {
            return Err(format!("Invalid channel name for {}", ch.id));
        }
        if !group_ids.contains(ch.group_id.as_str()) {
            return Err(format!("Channel {} references unknown group {}", ch.id, ch.group_id));
        }
        if !validate_stream_url(&ch.url) {
            return Err(format!("Unsafe or invalid stream URL for {}", ch.id));
        }
        if let Some(target) = ch.target_lufs
            && (!target.is_finite() || !(-36.0..=-5.0).contains(&target))
        {
            return Err(format!("Target loudness for {} must be -36.0..-5.0 LUFS", ch.id));
        }
        if let Some(tol) = ch.lufs_tolerance
            && (!tol.is_finite() || !(0.5..=10.0).contains(&tol))
        {
            return Err(format!("Loudness tolerance for {} must be 0.5..10.0 LU", ch.id));
        }
        if ch.broadcast_delay_secs > 7200 {
            return Err(format!(
                "Broadcast delay for {} must be 0..7200 seconds",
                ch.id
            ));
        }
    }
    Ok(())
}

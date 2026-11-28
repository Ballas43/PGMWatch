use super::xml::{parse_xml_root, XmlNode};
use super::MAX_SEGMENTS_PER_REPRESENTATION;
use anyhow::{anyhow, Context, Result};
use reqwest::Url;

#[derive(Debug, Clone)]
pub(super) struct DashSegment {
    pub(super) start_tick: u64,
    pub(super) duration_secs: f64,
    pub(super) url: String,
}

#[derive(Debug, Clone)]
pub(super) struct ParsedDashPlaylist {
    pub(super) target_duration: u64,
    pub(super) init_url: String,
    pub(super) segments: Vec<DashSegment>,
}

pub(crate) fn is_dash_mpd_ad_break(xml: &str) -> bool {
    let lower = xml.to_ascii_lowercase();
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

pub(super) fn format_hls_v7_playlist(parsed: &ParsedDashPlaylist, media_sequence: u64, ad_break: bool) -> String {
    let mut lines = Vec::with_capacity(7 + parsed.segments.len() * 2);
    lines.push("#EXTM3U".to_string());
    lines.push("#EXT-X-VERSION:7".to_string());
    lines.push(format!("#EXT-X-TARGETDURATION:{}", parsed.target_duration));
    lines.push(format!("#EXT-X-MEDIA-SEQUENCE:{}", media_sequence));
    if ad_break {
        lines.push("#EXT-X-CUE-OUT:30".to_string());
    }
    lines.push(format!(
        "#EXT-X-MAP:URI=\"{}\"",
        parsed.init_url.replace('"', "%22")
    ));
    for seg in &parsed.segments {
        lines.push(format!("#EXTINF:{:.3},", seg.duration_secs));
        lines.push(seg.url.clone());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Picks the best `(video_rep_id, audio_rep_id)` directly from a DASH MPD XML document
/// for fallback resolvers (such as Facebook Comet SSR `dash_manifest_urls`) that bypass yt-dlp.
pub fn pick_dash_video_audio_rep_ids(
    xml: &str,
    max_video_height: Option<u32>,
    audio_only: bool,
) -> (Option<String>, Option<String>) {
    let Some(root) = parse_xml_root(xml) else {
        return (None, None);
    };
    let Ok(dummy_base) = Url::parse("https://dash.local/manifest.mpd") else {
        return (None, None);
    };
    let mut candidates: Vec<RepEntry<'_>> = Vec::new();
    let periods = root.children_named("Period");
    if periods.is_empty() {
        let _ = collect_representations(&root, None, &dummy_base, &mut candidates);
    } else {
        // Prefer the latest <Period> first (live edge in multi-period dynamic MPDs).
        for period in periods.into_iter().rev() {
            let _ = collect_representations(period, None, &dummy_base, &mut candidates);
        }
    }

    let mut videos: Vec<(&str, u32, u64)> = Vec::new();
    let mut audios: Vec<(&str, u64)> = Vec::new();

    for c in &candidates {
        if c.rep_id.is_empty() {
            continue;
        }
        let mime = c
            .rep
            .attrs
            .get("mimeType")
            .or_else(|| c.adapt.and_then(|a| a.attrs.get("mimeType")))
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let bw: u64 = c
            .rep
            .attrs
            .get("bandwidth")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let height: u32 = c
            .rep
            .attrs
            .get("height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if mime.starts_with("audio/") || (height == 0 && c.rep_id.contains("-a")) {
            audios.push((c.rep_id, bw));
        } else if mime.starts_with("video/") || height > 0 {
            videos.push((c.rep_id, height, bw));
        }
    }

    let best_audio = audios
        .iter()
        .max_by_key(|(_, bw)| *bw)
        .map(|(id, _)| (*id).to_string());

    if audio_only {
        let fallback_low = best_audio.or_else(|| {
            videos
                .iter()
                .min_by_key(|(_, h, bw)| (*h, *bw))
                .map(|(id, _, _)| (*id).to_string())
        });
        return (fallback_low, None);
    }

    let best_video = if let Some(cap_h) = max_video_height {
        videos
            .iter()
            .filter(|(_, h, _)| *h > 0 && *h <= cap_h)
            .max_by_key(|(_, h, bw)| (*h, *bw))
            .or_else(|| videos.iter().min_by_key(|(_, h, bw)| (*h, *bw)))
            .map(|(id, _, _)| (*id).to_string())
    } else {
        videos
            .iter()
            .max_by_key(|(_, h, bw)| (*h, *bw))
            .map(|(id, _, _)| (*id).to_string())
    };

    (best_video, best_audio)
}

pub(super) fn parse_dash_representation_to_segments(
    xml: &str,
    mpd_url: &str,
    format_id: &str,
) -> Result<ParsedDashPlaylist> {
    let root = parse_xml_root(xml).ok_or_else(|| anyhow!("Empty or invalid MPD XML"))?;
    let mpd_base = Url::parse(mpd_url).with_context(|| format!("Invalid MPD URL: {mpd_url}"))?;
    let mpd_resolved = resolve_base_url(&mpd_base, &root)?;

    let mut candidates: Vec<RepEntry<'_>> = Vec::new();
    let periods = root.children_named("Period");
    if periods.is_empty() {
        collect_representations(&root, None, &mpd_resolved, &mut candidates)?;
    } else {
        // Iterate periods in reverse so dynamic multi-period MPDs select the live-edge period first.
        for period in periods.into_iter().rev() {
            let period_base = resolve_base_url(&mpd_resolved, period)?;
            collect_representations(period, None, &period_base, &mut candidates)?;
        }
    }

    if candidates.is_empty() {
        return Err(anyhow!("No DASH <Representation> elements found in MPD"));
    }

    let chosen = pick_representation(&candidates, format_id).ok_or_else(|| {
        let avail: Vec<&str> = candidates.iter().map(|c| c.rep_id).collect();
        anyhow!(
            "DASH representation '{}' not found in MPD (available: {})",
            format_id,
            avail.join(", ")
        )
    })?;

    let rep_tmpl = chosen.rep.find_child("SegmentTemplate");
    let adapt_tmpl = chosen
        .adapt
        .and_then(|a| a.find_child("SegmentTemplate"));
    if rep_tmpl.is_none() && adapt_tmpl.is_none() {
        return Err(anyhow!(
            "DASH representation '{}' has no <SegmentTemplate>",
            chosen.rep_id
        ));
    }

    let attr = |key: &str| -> Option<&str> {
        rep_tmpl
            .and_then(|t| t.attrs.get(key).map(|s| s.as_str()))
            .or_else(|| adapt_tmpl.and_then(|t| t.attrs.get(key).map(|s| s.as_str())))
    };

    let timescale: u64 = attr("timescale")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1);
    let start_number: u64 = attr("startNumber")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);

    let init_tmpl = attr("initialization")
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing initialization attribute"))?;
    let media_tmpl = attr("media")
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing media attribute"))?;

    let init_rel = replace_rep_id(init_tmpl, chosen.rep_id);
    let init_url = chosen
        .base
        .join(&init_rel)
        .with_context(|| format!("Failed resolving DASH init URL '{init_rel}'"))?
        .to_string();

    let timeline = rep_tmpl
        .and_then(|t| t.find_child("SegmentTimeline"))
        .or_else(|| adapt_tmpl.and_then(|t| t.find_child("SegmentTimeline")))
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing <SegmentTimeline>"))?;

    let mut segments = Vec::new();
    let mut cur_t: u64 = 0;
    let mut seg_number: u64 = start_number;
    let mut max_dur: f64 = 1.0;

    for s in timeline.children_named("S") {
        if let Some(t_str) = s.attrs.get("t")
            && let Ok(parsed_t) = t_str.parse::<u64>()
        {
            cur_t = parsed_t;
        }
        let d: u64 = s
            .attrs
            .get("d")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .ok_or_else(|| anyhow!("DASH <SegmentTimeline><S> missing valid duration 'd'"))?;
        let repeat_extra: usize = s
            .attrs
            .get("r")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|r| r.max(0) as usize)
            .unwrap_or(0)
            .min(MAX_SEGMENTS_PER_REPRESENTATION);

        let dur_secs = d as f64 / timescale as f64;
        if dur_secs > max_dur {
            max_dur = dur_secs;
        }

        for _ in 0..=repeat_extra {
            if segments.len() >= MAX_SEGMENTS_PER_REPRESENTATION {
                break;
            }
            let seg_rel = replace_rep_id(media_tmpl, chosen.rep_id)
                .replace("$Time$", &cur_t.to_string())
                .replace("$Number$", &seg_number.to_string());
            let seg_abs = chosen
                .base
                .join(&seg_rel)
                .with_context(|| format!("Failed resolving DASH segment URL '{seg_rel}'"))?
                .to_string();
            segments.push(DashSegment {
                start_tick: cur_t,
                duration_secs: dur_secs,
                url: seg_abs,
            });
            cur_t = cur_t.saturating_add(d);
            seg_number = seg_number.saturating_add(1);
        }
    }

    if segments.is_empty() {
        return Err(anyhow!("DASH <SegmentTimeline> produced 0 segments"));
    }

    Ok(ParsedDashPlaylist {
        target_duration: max_dur.ceil().max(1.0) as u64,
        init_url,
        segments,
    })
}

fn collect_representations<'a>(
    container: &'a XmlNode,
    enclosing_adapt: Option<&'a XmlNode>,
    base: &Url,
    out: &mut Vec<impl From<(&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)>>,
) -> Result<()> {
    for adapt in container.children_named("AdaptationSet") {
        let adapt_base = resolve_base_url(base, adapt)?;
        for rep in adapt.children_named("Representation") {
            let rep_base = resolve_base_url(&adapt_base, rep)?;
            let rep_id = rep.attrs.get("id").map(|s| s.as_str()).unwrap_or("");
            out.push((rep_id, rep, Some(adapt), rep_base).into());
        }
    }
    for rep in container.children_named("Representation") {
        let rep_base = resolve_base_url(base, rep)?;
        let rep_id = rep.attrs.get("id").map(|s| s.as_str()).unwrap_or("");
        out.push((rep_id, rep, enclosing_adapt, rep_base).into());
    }
    Ok(())
}

// Local conversion helper without exposing private Candidate outside parse_dash_representation_to_segments
struct RepEntry<'a> {
    rep_id: &'a str,
    rep: &'a XmlNode,
    adapt: Option<&'a XmlNode>,
    base: Url,
}

impl<'a> From<(&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)> for RepEntry<'a> {
    fn from(v: (&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)) -> Self {
        Self {
            rep_id: v.0,
            rep: v.1,
            adapt: v.2,
            base: v.3,
        }
    }
}

fn pick_representation<'a, T>(
    candidates: &'a [T],
    format_id: &str,
) -> Option<&'a T>
where
    T: HasRepId,
{
    if !format_id.is_empty() {
        if let Some(exact) = candidates.iter().find(|c| c.rep_id() == format_id) {
            return Some(exact);
        }
        // yt-dlp appends "-<n>" on duplicate Representation ids (e.g. "dash-lp-pst-v-2")
        if let Some(matched) = candidates
            .iter()
            .filter(|c| {
                let id = c.rep_id();
                !id.is_empty()
                    && format_id.starts_with(id)
                    && format_id.as_bytes().get(id.len()) == Some(&b'-')
                    && format_id[id.len() + 1..]
                        .chars()
                        .all(|ch| ch.is_ascii_digit())
            })
            .max_by_key(|c| c.rep_id().len())
        {
            return Some(matched);
        }
        return None;
    }
    candidates.first()
}

trait HasRepId {
    fn rep_id(&self) -> &str;
}

impl<'a> HasRepId for RepEntry<'a> {
    fn rep_id(&self) -> &str {
        self.rep_id
    }
}

fn replace_rep_id(template: &str, rep_id: &str) -> String {
    template.replace("$RepresentationID$", rep_id)
}

fn resolve_base_url(current: &Url, node: &XmlNode) -> Result<Url> {
    if let Some(base_el) = node.find_child("BaseURL") {
        let raw = base_el.text.trim();
        if !raw.is_empty() {
            return current
                .join(raw)
                .with_context(|| format!("Invalid <BaseURL> '{raw}'"));
        }
    }
    Ok(current.clone())
}

import type { AudioTelemetry, ChannelRuntimeSnapshot, TriStateMetric } from "../types";
import { ANALYTICS_GLOSSARY, type MetricGlossaryEntry } from "../utils/analyticsGlossary";
import { formatAvgLufs } from "./ChannelAudioMeter";
import { InfoTip } from "./InfoTip";
import { formatBitrate, formatTriStateCompact } from "../utils/telemetry";

interface ChannelDiagnosticsDrawerProps {
  id: string;
  channel: ChannelRuntimeSnapshot;
  audio: AudioTelemetry;
}

function MetricRow({
  label,
  value,
  tip,
}: {
  label: string;
  value: string;
  tip?: MetricGlossaryEntry;
}) {
  return (
    <div className="flex items-center justify-between gap-2 text-[11px]">
      {tip ? (
        <InfoTip entry={tip} underline focusable className="text-[var(--text-muted)]">
          {label}
        </InfoTip>
      ) : (
        <span className="text-[var(--text-muted)]">{label}</span>
      )}
      <span className="font-mono-tabular text-[var(--text-primary)]">{value}</span>
    </div>
  );
}

function SocialAuditRow({
  label,
  metric,
  tip,
}: {
  label: string;
  metric: TriStateMetric;
  tip: MetricGlossaryEntry;
}) {
  const sourceTag = metric.source === "MANUAL_STUDIO" ? "Studio" : "Auto";
  return (
    <div
      className="flex items-center justify-between gap-2 text-[11px]"
      title={metric.note ?? `${label} (${metric.source})`}
    >
      <InfoTip entry={tip} underline focusable className="text-[var(--text-muted)]">
        {label}
      </InfoTip>
      <span className="font-mono-tabular text-[var(--text-secondary)]">
        <InfoTip entry={ANALYTICS_GLOSSARY.metricStatus} focusable>
          <span className="text-[var(--text-primary)]">{formatTriStateCompact(metric)}</span>
        </InfoTip>
        {" · "}
        <InfoTip entry={ANALYTICS_GLOSSARY.coverage} underline focusable>
          {metric.coverage_pct.toFixed(0)}%
        </InfoTip>
        {" · "}
        <InfoTip entry={ANALYTICS_GLOSSARY.metricSource} underline focusable>
          {sourceTag}
        </InfoTip>
      </span>
    </div>
  );
}

export function ChannelDiagnosticsDrawer({
  id,
  channel,
  audio,
}: ChannelDiagnosticsDrawerProps) {
  const { video } = channel;
  const changedPct = video.changed_pct ?? 0;
  const lumaVar = video.luma_variance ?? 0;
  const blockiness = video.blockiness_score ?? 0;

  return (
    <div
      id={id}
      className="mt-2.5 space-y-2.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-rail)]/80 p-2.5"
    >
      <div>
        <div className="mb-1 text-[11px] font-semibold text-[var(--text-secondary)]">
          Video & Stream Metrics
        </div>
        <div className="grid grid-cols-2 gap-x-3 gap-y-1">
          <MetricRow
            label="Resolution"
            tip={ANALYTICS_GLOSSARY.resolutionFps}
            value={video.width > 0 ? `${video.width}×${video.height}@${video.fps.toFixed(0)}` : "Audio Only"}
          />
          <MetricRow label="Bitrate" tip={ANALYTICS_GLOSSARY.resolutionFps} value={formatBitrate(video.bitrate_kbps)} />
          <MetricRow label="Compression (QP)" tip={ANALYTICS_GLOSSARY.avgQp} value={video.avg_qp.toFixed(1)} />
          <MetricRow label="Decode Time" tip={ANALYTICS_GLOSSARY.decodeMs} value={`${video.decode_ms.toFixed(2)} ms`} />
          <MetricRow
            label="Audio/Video Sync"
            tip={ANALYTICS_GLOSSARY.avSync}
            value={`${video.av_sync_ms >= 0 ? "+" : ""}${video.av_sync_ms.toFixed(1)} ms`}
          />
          <MetricRow label="CDN Load" tip={ANALYTICS_GLOSSARY.cdnRatio} value={`${(video.cdn_ratio * 100).toFixed(0)}%`} />
          <MetricRow label="Screen Motion" tip={ANALYTICS_GLOSSARY.motionPct} value={`${changedPct.toFixed(1)}%`} />
          <MetricRow label="Blockiness" tip={ANALYTICS_GLOSSARY.blockiness} value={blockiness.toFixed(2)} />
          <MetricRow label="Brightness" tip={ANALYTICS_GLOSSARY.lumaMean} value={video.luma_mean.toFixed(1)} />
          <MetricRow label="Contrast Spread" tip={ANALYTICS_GLOSSARY.lumaVar} value={lumaVar.toFixed(1)} />
          <MetricRow label="Frame Change" tip={ANALYTICS_GLOSSARY.frameMad} value={video.frame_mad.toFixed(2)} />
          <MetricRow label="Stream Engine" tip={ANALYTICS_GLOSSARY.resolver} value={channel.resolved_via ?? "pending"} />
        </div>
      </div>

      <div className="border-t border-[var(--border-hairline)] pt-2">
        <div className="mb-1 text-[11px] font-semibold text-[var(--text-secondary)]">
          Audio & Loudness Metrics
        </div>
        <div className="grid grid-cols-2 gap-x-3 gap-y-1">
          <MetricRow label="Avg Loudness" tip={ANALYTICS_GLOSSARY.avgLufs} value={`${formatAvgLufs(audio.integrated_lufs)} LUFS`} />
          <MetricRow label="Short Loudness" tip={ANALYTICS_GLOSSARY.shortLufs} value={`${formatAvgLufs(audio.short_term_lufs)} LUFS`} />
          <MetricRow label="True Peak" tip={ANALYTICS_GLOSSARY.truePeak} value={`${audio.true_peak_dbtp.toFixed(1)} dBTP`} />
          <MetricRow label="Stereo Phase" tip={ANALYTICS_GLOSSARY.phaseCorr} value={audio.phase_correlation.toFixed(2)} />
          <MetricRow
            label="Power Hum"
            tip={ANALYTICS_GLOSSARY.mainsHum}
            value={
              audio.hum_freq_hz !== null
                ? `${Math.round(audio.hum_freq_hz)}Hz (${audio.hum_peak_db.toFixed(1)}dB)`
                : `Clean (${audio.hum_peak_db.toFixed(1)}dB)`
            }
          />
          <MetricRow label="Uptime" tip={ANALYTICS_GLOSSARY.uptime} value={`${channel.uptime_pct.toFixed(1)}%`} />
        </div>
      </div>

      <div className="border-t border-[var(--border-hairline)] pt-2">
        <div className="mb-1 text-[11px] font-semibold text-[var(--text-secondary)]">
          Viewer & Social Sources
        </div>
        <div className="grid grid-cols-1 gap-1">
          <SocialAuditRow label="Live Viewers" tip={ANALYTICS_GLOSSARY.ccv} metric={channel.ccv} />
          <SocialAuditRow label="Chat/min" tip={ANALYTICS_GLOSSARY.chatRate} metric={channel.chat_rate_per_min} />
          <SocialAuditRow label="Views" tip={ANALYTICS_GLOSSARY.views} metric={channel.total_views} />
          <SocialAuditRow label="Likes" tip={ANALYTICS_GLOSSARY.likes} metric={channel.likes} />
          <SocialAuditRow label="Shares" tip={ANALYTICS_GLOSSARY.shares} metric={channel.shares} />
          <SocialAuditRow label="Gifts" tip={ANALYTICS_GLOSSARY.gifts} metric={channel.gifts_count} />
        </div>
      </div>
    </div>
  );
}

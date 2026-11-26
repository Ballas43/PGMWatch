import { AlertTriangle, Cpu, MessageSquareWarning, Radio } from "lucide-react";
import type { ChannelRuntimeSnapshot } from "../types";
import { ANALYTICS_GLOSSARY, FAULT_GLOSSARY, type MetricGlossaryEntry } from "../utils/analyticsGlossary";
import {
  formatBitrate,
  formatCompactNumber,
  formatFaultLabel,
  isCriticalFault,
} from "../utils/telemetry";
import { InfoTip } from "./InfoTip";

interface ChannelSignalStripProps {
  channel: ChannelRuntimeSnapshot;
}

export function ChannelSignalStrip({ channel }: ChannelSignalStripProps) {
  const { video, active_faults, effective_tier, config } = channel;
  const isLight = effective_tier === "LIGHT";
  const cdnPct = Math.round(Math.max(0, video.cdn_ratio) * 100);
  const absSync = Math.abs(video.av_sync_ms);

  const syncClass =
    absSync >= 100
      ? "text-[var(--negative)]"
      : absSync >= 45
      ? "text-[var(--warning)]"
      : "text-[var(--text-secondary)]";

  const cdnClass =
    cdnPct >= 85
      ? "text-[var(--negative)]"
      : cdnPct >= 65
      ? "text-[var(--warning)]"
      : "text-[var(--text-muted)]";

  const chatSpike = active_faults.includes("CHAT_COMPLAINT_SPIKE");

  const socialItems: { label: string; value: number; spike?: boolean; tip: MetricGlossaryEntry }[] = [];
  if (channel.chat_rate_per_min.status === "VALUE" && channel.chat_rate_per_min.value !== null) {
    socialItems.push({
      label: "Chat/min",
      value: channel.chat_rate_per_min.value,
      spike: chatSpike,
      tip: ANALYTICS_GLOSSARY.chatRate,
    });
  }
  if (channel.likes.status === "VALUE" && channel.likes.value !== null) {
    socialItems.push({ label: "Likes", value: channel.likes.value, tip: ANALYTICS_GLOSSARY.likes });
  }
  if (channel.total_views.status === "VALUE" && channel.total_views.value !== null) {
    socialItems.push({ label: "Views", value: channel.total_views.value, tip: ANALYTICS_GLOSSARY.views });
  }
  if (channel.shares.status === "VALUE" && channel.shares.value !== null) {
    socialItems.push({ label: "Shares", value: channel.shares.value, tip: ANALYTICS_GLOSSARY.shares });
  }
  if (channel.gifts_count.status === "VALUE" && channel.gifts_count.value !== null) {
    socialItems.push({ label: "Gifts", value: channel.gifts_count.value, tip: ANALYTICS_GLOSSARY.gifts });
  }

  return (
    <div className="mt-2.5 space-y-2">
      {/* Compact Video / Decode / CDN Transport Telemetry */}
      <div className="flex flex-wrap items-center justify-between gap-x-2 gap-y-1 rounded-md border border-[var(--border-hairline)] bg-[var(--bg-raised)]/50 px-2 py-1 text-[11px] font-mono-tabular">
        <div className="flex flex-wrap items-center gap-1.5 text-[var(--text-secondary)]">
          <InfoTip entry={ANALYTICS_GLOSSARY.hwDecode} focusable>
            <span
              className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium ${
                video.hw_accelerated
                  ? "border border-[#4cc38a]/40 bg-[#4cc38a]/10 text-[#4cc38a]"
                  : "border border-[var(--border-hairline)] bg-[var(--bg-rail)] text-[var(--text-muted)]"
              }`}
            >
              {video.hw_accelerated ? (
                <Cpu size={10} strokeWidth={1.5} aria-hidden="true" />
              ) : (
                <Radio size={10} strokeWidth={1.5} aria-hidden="true" />
              )}
              <span>{video.hw_accelerated ? "GPU" : "CPU"}</span>
            </span>
          </InfoTip>

          <InfoTip entry={ANALYTICS_GLOSSARY.resolutionFps} underline focusable className="inline-flex items-center gap-1.5">
            {!isLight && video.width > 0 && video.height > 0 && (
              <span>
                {video.height}p{Math.round(video.fps)}
              </span>
            )}
            <span>{formatBitrate(video.bitrate_kbps)}</span>
          </InfoTip>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          {config.enabled && (
            <>
              {!isLight && (
                <>
                  <InfoTip entry={ANALYTICS_GLOSSARY.avSync} underline focusable>
                    <span className={syncClass}>
                      Sync {video.av_sync_ms >= 0 ? "+" : ""}
                      {Math.round(video.av_sync_ms)}ms
                    </span>
                  </InfoTip>
                  <InfoTip entry={ANALYTICS_GLOSSARY.decodeMs} underline focusable>
                    <span className="text-[var(--text-muted)]">
                      Decode {video.decode_ms.toFixed(1)}ms
                    </span>
                  </InfoTip>
                </>
              )}
              <InfoTip entry={ANALYTICS_GLOSSARY.cdnRatio} underline focusable>
                <span className={cdnClass}>
                  CDN Load {cdnPct}%
                </span>
              </InfoTip>
            </>
          )}
        </div>
      </div>

      {/* Live Social & Chat Signals (omits N/A fields) */}
      {socialItems.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 text-[11px] font-mono-tabular">
          {socialItems.map((item) => (
            <InfoTip key={item.label} entry={item.tip} focusable>
              <span
                className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 ${
                  item.spike
                    ? "border-[var(--warning)]/60 bg-[var(--warning-soft)] text-[var(--warning-text)]"
                    : "border-[var(--border-hairline)] bg-[var(--bg-page)] text-[var(--text-secondary)]"
                }`}
              >
                {item.spike && <MessageSquareWarning size={10} strokeWidth={1.5} aria-hidden="true" />}
                <span className="text-[var(--text-muted)]">{item.label}</span>
                <span className="text-[var(--text-primary)]">{formatCompactNumber(item.value)}</span>
              </span>
            </InfoTip>
          ))}
        </div>
      )}

      {/* Simultaneous Active Fault Pills */}
      {active_faults.length > 0 && (
        <div className="flex flex-wrap items-center gap-1" role="status" aria-live="polite">
          {active_faults.map((fault) => {
            const critical = isCriticalFault(fault);
            return (
              <InfoTip key={fault} entry={FAULT_GLOSSARY[fault]} focusable>
                <span
                  className={`inline-flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px] font-medium ${
                    critical
                      ? "border-[var(--negative)]/50 bg-[var(--negative-soft)] text-[var(--negative-text)]"
                      : "border-[var(--warning)]/50 bg-[var(--warning-soft)] text-[var(--warning-text)]"
                  }`}
                >
                  <AlertTriangle size={10} strokeWidth={1.5} aria-hidden="true" />
                  <span>{formatFaultLabel(fault)}</span>
                </span>
              </InfoTip>
            );
          })}
        </div>
      )}
    </div>
  );
}

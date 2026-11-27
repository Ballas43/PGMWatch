import { useCallback, useEffect, useState } from "react";
import { Check, HardDrive, Play, Trash2 } from "lucide-react";
import { ClipStorageStats, DashboardSnapshot, IncidentRecord } from "../types";
import { ANALYTICS_GLOSSARY, FAULT_GLOSSARY } from "../utils/analyticsGlossary";
import { formatFaultLabel, isBookmarkFault, isCriticalFault } from "../utils/telemetry";
import { formatLocalDateTime, getShortTimeZoneName } from "../utils/time";
import { ClipModal } from "./ClipModal";
import { InfoTip } from "./InfoTip";

interface ClipsIncidentsViewProps {
  snapshot: DashboardSnapshot | null;
  onAck: (channelId: string) => void;
  onRefresh?: () => void;
}

function formatClipBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 MB";
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function ClipsIncidentsView({ snapshot, onAck, onRefresh }: ClipsIncidentsViewProps) {
  const [selectedClip, setSelectedClip] = useState<IncidentRecord | null>(null);
  const [filter, setFilter] = useState<"all" | "open" | "with_clip" | "signal_chat">("all");
  const [clipStats, setClipStats] = useState<ClipStorageStats | null>(null);
  const [confirmClearClips, setConfirmClearClips] = useState(false);
  const [clearingClips, setClearingClips] = useState(false);
  const [confirmClearLog, setConfirmClearLog] = useState(false);
  const [clearingLog, setClearingLog] = useState(false);
  const [confirmFlushAll, setConfirmFlushAll] = useState(false);
  const [flushingAll, setFlushingAll] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const loadClipStats = useCallback(async () => {
    try {
      const res = await fetch("/api/clips/stats");
      if (res.ok) {
        setClipStats(await res.json());
      }
    } catch {
      // ignore transient poll failures
    }
  }, []);

  const allIncidents = snapshot?.incidents ?? [];
  const incidentClipCount = allIncidents.filter((inc) => Boolean(inc.clip_filename)).length;
  const resolvedCount = allIncidents.filter((inc) => inc.resolved_at !== null).length;

  useEffect(() => {
    void loadClipStats();
    const timer = window.setInterval(() => void loadClipStats(), 10000);
    return () => window.clearInterval(timer);
  }, [loadClipStats, allIncidents.length, incidentClipCount]);

  const showNotice = (msg: string) => {
    setActionError(null);
    setNotice(msg);
    window.setTimeout(() => setNotice(null), 4000);
  };

  const handleClearClips = async () => {
    if (!confirmClearClips) {
      setConfirmClearClips(true);
      window.setTimeout(() => setConfirmClearClips(false), 4000);
      return;
    }
    setClearingClips(true);
    try {
      const res = await fetch("/api/clips/clear", { method: "POST" });
      if (!res.ok) {
        setActionError(`Failed to clear clips (HTTP ${res.status})`);
        return;
      }
      const data = (await res.json()) as {
        deleted_files?: number;
        freed_bytes?: number;
        stats?: ClipStorageStats;
      };
      setConfirmClearClips(false);
      setSelectedClip(null);
      if (data.stats) setClipStats(data.stats);
      else await loadClipStats();
      showNotice(
        `Cleared ${data.deleted_files ?? 0} MP4 clip${data.deleted_files === 1 ? "" : "s"} (${formatClipBytes(data.freed_bytes ?? 0)} freed)`
      );
      onRefresh?.();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Network error clearing clips");
    } finally {
      setClearingClips(false);
    }
  };

  const handleClearResolvedLog = async () => {
    if (!confirmClearLog) {
      setConfirmClearLog(true);
      window.setTimeout(() => setConfirmClearLog(false), 4000);
      return;
    }
    setClearingLog(true);
    try {
      const res = await fetch("/api/incidents/clear", { method: "POST" });
      if (!res.ok) {
        setActionError(`Failed to clear resolved log (HTTP ${res.status})`);
        return;
      }
      const data = (await res.json()) as {
        removed_incidents?: number;
        freed_bytes?: number;
        stats?: ClipStorageStats;
      };
      setConfirmClearLog(false);
      if (data.stats) setClipStats(data.stats);
      else await loadClipStats();
      showNotice(
        `Cleared ${data.removed_incidents ?? 0} resolved log entr${data.removed_incidents === 1 ? "y" : "ies"}`
      );
      onRefresh?.();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Network error clearing log");
    } finally {
      setClearingLog(false);
    }
  };

  const handleFlushAllIncidents = async () => {
    if (!confirmFlushAll) {
      setConfirmFlushAll(true);
      window.setTimeout(() => setConfirmFlushAll(false), 4000);
      return;
    }
    setFlushingAll(true);
    try {
      const res = await fetch("/api/incidents/flush", { method: "POST" });
      if (!res.ok) {
        setActionError(`Failed to flush incidents (HTTP ${res.status})`);
        return;
      }
      const data = (await res.json()) as {
        removed_incidents?: number;
        freed_bytes?: number;
        stats?: ClipStorageStats;
      };
      setConfirmFlushAll(false);
      setSelectedClip(null);
      if (data.stats) setClipStats(data.stats);
      else await loadClipStats();
      showNotice(
        `Flushed ${data.removed_incidents ?? 0} incident${data.removed_incidents === 1 ? "" : "s"} (${formatClipBytes(data.freed_bytes ?? 0)} freed)`
      );
      onRefresh?.();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Network error flushing incidents");
    } finally {
      setFlushingAll(false);
    }
  };

  const handleDeleteSingleClip = async (filename: string): Promise<boolean> => {
    setActionError(null);
    try {
      const res = await fetch(`/api/clips/${encodeURIComponent(filename)}/delete`, {
        method: "POST",
      });
      if (!res.ok) {
        setActionError(`Failed to delete clip (HTTP ${res.status})`);
        return false;
      }
      const data = (await res.json()) as {
        freed_bytes?: number;
        stats?: ClipStorageStats;
      };
      if (data.stats) setClipStats(data.stats);
      else await loadClipStats();
      showNotice(`Deleted ${filename} (${formatClipBytes(data.freed_bytes ?? 0)} freed)`);
      onRefresh?.();
      return true;
    } catch (err) {
      setActionError(err instanceof Error ? err.message : "Network error deleting clip");
      return false;
    }
  };

  const storedFilesCount = clipStats?.clip_count ?? incidentClipCount;
  const hasAnyClips = storedFilesCount > 0 || incidentClipCount > 0;
  const incidents = allIncidents.filter((inc) => {
    if (filter === "open") return !inc.acknowledged || inc.resolved_at === null;
    if (filter === "with_clip") return Boolean(inc.clip_filename);
    if (filter === "signal_chat") {
      return (
        inc.fault === "CHAT_COMPLAINT_SPIKE" ||
        inc.fault === "AV_SYNC_DRIFT" ||
        inc.fault === "PTS_DRIFT" ||
        inc.fault === "VIDEO_MACROBLOCKING" ||
        inc.fault === "AUDIO_MAINS_HUM"
      );
    }
    return true;
  });

  return (
    <div className="space-y-4 animate-enter">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <h1 className="text-lg font-medium tracking-tight text-[var(--text-primary)]">
            Clips & Incident Log
          </h1>
          <p className="text-xs text-[var(--text-secondary)]">
            Instant MP4 clips saved from the last 2–6s of live video (oldest clips auto-delete when storage is full)
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <div className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2.5 text-xs font-mono-tabular text-[var(--text-secondary)]">
            <HardDrive size={13} strokeWidth={1.5} className="shrink-0 text-[var(--text-muted)]" />
            <InfoTip entry={ANALYTICS_GLOSSARY.clipStorage} underline focusable>
              <span>
                {storedFilesCount} MP4{storedFilesCount === 1 ? "" : "s"} ·{" "}
                {formatClipBytes(clipStats?.total_bytes ?? 0)} / {(clipStats?.quota_gb ?? 10).toFixed(1)} GB max
              </span>
            </InfoTip>
          </div>

          <button
            type="button"
            disabled={clearingClips || !hasAnyClips}
            onClick={() => void handleClearClips()}
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
              confirmClearClips
                ? "border-[var(--negative)] bg-[var(--negative-soft)] text-[var(--negative-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            }`}
          >
            <Trash2 size={13} strokeWidth={1.5} />
            <span>
              {clearingClips
                ? "Clearing..."
                : confirmClearClips
                ? `Confirm Delete (${storedFilesCount})`
                : "Clear Clips"}
            </span>
          </button>

          <button
            type="button"
            disabled={clearingLog || resolvedCount === 0}
            onClick={() => void handleClearResolvedLog()}
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
              confirmClearLog
                ? "border-[var(--negative)] bg-[var(--negative-soft)] text-[var(--negative-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            }`}
          >
            <span>
              {clearingLog
                ? "Clearing..."
                : confirmClearLog
                ? `Confirm Clear (${resolvedCount})`
                : "Clear Resolved"}
            </span>
          </button>

          <button
            type="button"
            disabled={flushingAll || allIncidents.length === 0}
            onClick={() => void handleFlushAllIncidents()}
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
              confirmFlushAll
                ? "border-[var(--negative)] bg-[var(--negative-soft)] text-[var(--negative-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:border-[var(--negative)]/50 hover:text-[var(--negative-text)]"
            }`}
          >
            <span>
              {flushingAll
                ? "Flushing..."
                : confirmFlushAll
                ? `Confirm Flush All (${allIncidents.length})`
                : "Flush All"}
            </span>
          </button>

          <div className="flex flex-wrap items-center gap-1 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] p-1 text-xs">
            {(
              [
                ["all", "All Events"],
                ["open", "Unacknowledged"],
                ["with_clip", "With Clip"],
                ["signal_chat", "A/V & Chat Spikes"],
              ] as const
            ).map(([key, label]) => (
              <button
                key={key}
                type="button"
                onClick={() => setFilter(key)}
                className={`rounded-md px-2.5 py-1 transition-colors ${
                  filter === key
                    ? "bg-[var(--bg-raised)] font-medium text-[var(--text-primary)]"
                    : "text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                }`}
              >
                {label}
              </button>
            ))}
          </div>
        </div>
      </div>

      {actionError && (
        <div
          role="alert"
          className="rounded-lg border border-[var(--negative)]/40 bg-[var(--negative-soft)] px-3.5 py-2 text-xs text-[var(--negative-text)]"
        >
          {actionError}
        </div>
      )}

      {notice && (
        <div
          role="status"
          className="rounded-lg border border-[var(--positive)]/40 bg-[var(--positive-soft)] px-3.5 py-2 text-xs text-[var(--positive-text)]"
        >
          {notice}
        </div>
      )}

      {incidents.length === 0 ? (
        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-12 text-center">
          <h2 className="text-base font-medium text-[var(--text-primary)]">No incidents recorded</h2>
          <p className="mt-1 text-sm text-[var(--text-secondary)]">
            Trigger Force Clip on any monitor card or inject a simulated fault from Show Setup to test capture.
          </p>
        </div>
      ) : (
        <div className="overflow-x-auto rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-4">
          <table className="w-full text-left text-xs">
            <thead>
              <tr className="border-b border-[var(--border-hairline)] text-[var(--text-muted)]">
                <th scope="col" className="pb-2.5 pr-3 font-medium">
                  <InfoTip entry={ANALYTICS_GLOSSARY.gridBucket} underline focusable>
                    Started ({getShortTimeZoneName()})
                  </InfoTip>
                </th>
                <th scope="col" className="pb-2.5 pr-3 font-medium">Channel</th>
                <th scope="col" className="pb-2.5 pr-3 font-medium">Type</th>
                <th scope="col" className="pb-2.5 pr-3 font-medium">Detail</th>
                <th scope="col" className="pb-2.5 pr-3 font-medium">Severity</th>
                <th scope="col" className="pb-2.5 pr-3 font-medium text-right">Duration</th>
                <th scope="col" className="pb-2.5 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-hairline)] font-mono-tabular">
              {incidents.map((inc) => (
                <tr key={inc.id} className="hover:bg-[var(--bg-raised)]/40">
                  <td className="py-3 pr-3 whitespace-nowrap text-[var(--text-secondary)]">
                    {formatLocalDateTime(inc.started_at)}
                  </td>
                  <td className="py-3 pr-3 font-sans font-medium text-[var(--text-primary)]">
                    {inc.channel_name}
                  </td>
                  <td className="py-3 pr-3 font-sans">
                    <InfoTip entry={FAULT_GLOSSARY[inc.fault]} focusable>
                      <span
                        className={`inline-flex rounded border px-1.5 py-0.5 text-[11px] font-medium whitespace-nowrap ${
                          isBookmarkFault(inc.fault)
                            ? "border-[var(--accent)]/40 bg-[var(--accent-soft)] text-[var(--accent-text)]"
                            : isCriticalFault(inc.fault)
                            ? "border-[var(--negative)]/40 bg-[var(--negative-soft)] text-[var(--negative-text)]"
                            : "border-[var(--warning)]/40 bg-[var(--warning-soft)] text-[var(--warning-text)]"
                        }`}
                      >
                        {formatFaultLabel(inc.fault)}
                      </span>
                    </InfoTip>
                  </td>
                  <td className="py-3 pr-3 font-sans text-[var(--text-primary)]">{inc.detail}</td>
                  <td className="py-3 pr-3">
                    <span
                      className={
                        inc.severity === "CRITICAL"
                          ? "text-[var(--negative-text)]"
                          : inc.severity === "WARNING"
                          ? "text-[var(--warning-text)]"
                          : "text-[var(--accent-text)]"
                      }
                    >
                      {isBookmarkFault(inc.fault) ? "NOTE" : inc.severity}
                    </span>
                  </td>
                  <td className="py-3 pr-3 text-right text-[var(--text-secondary)]">
                    {inc.duration_secs.toFixed(1)}s
                  </td>
                  <td className="py-3 text-right">
                    <div className="inline-flex items-center gap-1.5">
                      {inc.clip_filename && (
                        <button
                          type="button"
                          onClick={() => setSelectedClip(inc)}
                          className="inline-flex h-7 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs font-sans text-[var(--text-primary)] hover:border-[var(--border-strong)]"
                        >
                          <Play size={12} strokeWidth={1.5} />
                          <span>Replay</span>
                        </button>
                      )}
                      {!inc.acknowledged && (
                        <button
                          type="button"
                          onClick={() => onAck(inc.channel_id)}
                          className="inline-flex h-7 items-center gap-1 rounded bg-[#e5484d] px-2 text-xs font-sans text-white"
                        >
                          <Check size={12} strokeWidth={1.5} />
                          <span>Ack</span>
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <ClipModal
        clipFilename={selectedClip?.clip_filename ?? null}
        title={selectedClip ? `${selectedClip.channel_name} — ${selectedClip.detail}` : ""}
        onClose={() => setSelectedClip(null)}
        onDeleteClip={handleDeleteSingleClip}
      />
    </div>
  );
}

import { useCallback, useEffect, useRef, useState } from "react";
import { BookmarkPlus, CheckCheck, MonitorPlay, PauseCircle, PlayCircle, RefreshCw, RotateCcw } from "lucide-react";
import { AudioTelemetry, DashboardSnapshot } from "../types";
import { StereoMeterSample } from "../utils/audioAnalyser";
import { BrandLogo } from "./BrandLogo";
import { canChannelUseHq, ChannelCard } from "./ChannelCard";
import { GovernorStatusStrip } from "./GovernorStatusStrip";

interface LiveWallViewProps {
  snapshot: DashboardSnapshot | null;
  thumbs: Record<string, string>;
  meters: Record<string, AudioTelemetry>;
  globalPflMuted: boolean;
  pcmPflMeter: StereoMeterSample | null;
  onMseAudioChange: (id: string, hasMseAudio: boolean) => void;
  onAck: (id: string) => void;
  onAckAll: () => Promise<void> | void;
  onRetryOffline: () => Promise<void> | void;
  onToggleChannelEnabled: (id: string, enabled: boolean) => Promise<void> | void;
  onAddBookmark: (channelId: string | null, note: string) => Promise<void> | void;
  onForceClip: (id: string) => void;
  onTogglePfl: (id: string) => void;
  onResetLufs: (id: string) => Promise<void> | void;
  onSetLufsTarget: (
    id: string,
    targetLufs: number | null,
    lufsTolerance: number | null
  ) => Promise<void> | void;
  onResetAllLufs: () => Promise<void> | void;
  onToggleGroupIntermission: (groupId: string, active: boolean) => void;
  onLoginFallback: (id: string) => void;
  onWipeAuth: (id: string) => void;
  onRetryChannel: (id: string) => void;
  onNavigateSetup: () => void;
}

export function LiveWallView({
  snapshot,
  thumbs,
  meters,
  globalPflMuted,
  pcmPflMeter,
  onMseAudioChange,
  onAck,
  onAckAll,
  onRetryOffline,
  onToggleChannelEnabled,
  onAddBookmark,
  onForceClip,
  onTogglePfl,
  onResetLufs,
  onSetLufsTarget,
  onResetAllLufs,
  onToggleGroupIntermission,
  onLoginFallback,
  onWipeAuth,
  onRetryChannel,
  onNavigateSetup,
}: LiveWallViewProps) {
  const [resettingAllLufs, setResettingAllLufs] = useState(false);
  const [clearedAllLufs, setClearedAllLufs] = useState(false);
  const [noteOpen, setNoteOpen] = useState(false);
  const [noteChannelId, setNoteChannelId] = useState<string>("");
  const [noteText, setNoteText] = useState("");
  const [savingNote, setSavingNote] = useState(false);
  const [hqByChannel, setHqByChannel] = useState<Record<string, boolean>>({});
  const resetTimerRef = useRef<number | null>(null);

  const handleSetChannelHq = useCallback((id: string, next: boolean) => {
    setHqByChannel((prev) => {
      if (Boolean(prev[id]) === next) return prev;
      return { ...prev, [id]: next };
    });
  }, []);

  const setBatchHq = useCallback((ids: string[], enabled: boolean) => {
    setHqByChannel((prev) => {
      const next = { ...prev };
      for (const id of ids) {
        next[id] = enabled;
      }
      return next;
    });
  }, []);

  useEffect(() => {
    return () => {
      if (resetTimerRef.current !== null) {
        window.clearTimeout(resetTimerRef.current);
      }
    };
  }, []);

  const handleResetAll = async () => {
    if (resettingAllLufs) return;
    setResettingAllLufs(true);
    try {
      await onResetAllLufs();
      setClearedAllLufs(true);
      if (resetTimerRef.current !== null) {
        window.clearTimeout(resetTimerRef.current);
      }
      resetTimerRef.current = window.setTimeout(() => setClearedAllLufs(false), 1200);
    } finally {
      setResettingAllLufs(false);
    }
  };
  if (!snapshot) {
    return (
      <div className="space-y-4" role="status" aria-busy="true" aria-label="Loading live feeds">
        <div className="h-8 w-48 rounded bg-[var(--bg-raised)]" />
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
          {[1, 2, 3, 4].map((i) => (
            <div key={i} className="h-64 rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)]" />
          ))}
        </div>
      </div>
    );
  }

  if (snapshot.channels.length === 0) {
    return (
      <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-12 text-center">
        <BrandLogo size={42} variant="badge" className="mx-auto mb-3" />
        <h1 className="text-xl font-medium text-[var(--text-primary)]">No active broadcast feeds</h1>
        <p className="mt-2 text-sm text-[var(--text-secondary)]">
          Add public stream URLs or load a Show JSON preset to start confidence monitoring.
        </p>
        <button
          type="button"
          onClick={onNavigateSetup}
          className="mt-4 inline-flex h-9 items-center rounded-lg bg-[var(--text-primary)] px-4 text-sm font-medium text-[var(--bg-rail)]"
        >
          Configure Show Feeds
        </button>
      </div>
    );
  }

  const unackedAlertCount = snapshot.channels.filter(
    (c) => c.alert_state === "WARNING" || c.alert_state === "CRITICAL"
  ).length;
  const offlineRetryCount = snapshot.channels.filter(
    (c) =>
      c.config.enabled &&
      (c.alert_state === "OFFLINE" ||
        c.alert_state === "EXTRACTOR_ERR" ||
        c.alert_state === "LOGIN_GATED")
  ).length;
  const eligibleHqIds = snapshot.channels.filter(canChannelUseHq).map((c) => c.config.id);
  const activeHqCount = eligibleHqIds.filter((id) => Boolean(hqByChannel[id])).length;
  const allGlobalHq = eligibleHqIds.length > 0 && activeHqCount === eligibleHqIds.length;

  const submitBookmark = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = noteText.trim();
    if (!trimmed || savingNote) return;
    setSavingNote(true);
    try {
      await onAddBookmark(noteChannelId || null, trimmed);
      setNoteText("");
      setNoteOpen(false);
    } finally {
      setSavingNote(false);
    }
  };

  return (
    <div className="space-y-8 animate-enter">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-medium tracking-tight text-[var(--text-primary)]">
            Live Confidence Wall
          </h1>
          <p className="mt-1 text-sm text-[var(--text-secondary)]">
            Live broadcast monitoring normalized at 48 kHz with ~1.5–2.5s keyframe fault clips
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div className="mr-1 text-xs font-mono-tabular text-[var(--text-muted)]">
            {snapshot.channels.length} channels · {snapshot.show.groups.length} groups
          </div>
          {unackedAlertCount > 0 && (
            <button
              type="button"
              onClick={() => void onAckAll()}
              className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--negative)]/50 bg-[var(--negative-soft)] px-3 text-xs font-medium text-[var(--negative-text)] transition-colors hover:bg-[var(--negative)]/25"
            >
              <CheckCheck size={13} strokeWidth={1.5} aria-hidden="true" />
              <span>Acknowledge All ({unackedAlertCount})</span>
            </button>
          )}
          {offlineRetryCount > 0 && (
            <button
              type="button"
              onClick={() => void onRetryOffline()}
              className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-3 text-xs font-medium text-[var(--text-secondary)] transition-colors hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
              title="Re-acquire all offline or errored streams"
            >
              <RefreshCw size={13} strokeWidth={1.5} aria-hidden="true" />
              <span>Retry Offline ({offlineRetryCount})</span>
            </button>
          )}
          {eligibleHqIds.length > 0 && (
            <button
              type="button"
              aria-pressed={allGlobalHq}
              onClick={() => setBatchHq(eligibleHqIds, !allGlobalHq)}
              title={
                allGlobalHq
                  ? "All eligible feeds are playing 60p HQ. Click to switch all back to 1.5fps thumbnails."
                  : "Switch all eligible feeds to 60p HQ (MSE) live video playback. Higher GPU/RAM usage."
              }
              className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs font-medium transition-colors ${
                allGlobalHq
                  ? "border-[#4cc38a]/50 bg-[#4cc38a]/12 text-[#4cc38a] hover:bg-[#4cc38a]/20"
                  : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
              }`}
            >
              <MonitorPlay size={13} strokeWidth={1.5} aria-hidden="true" />
              <span>
                {allGlobalHq
                  ? "Preview All (1.5fps)"
                  : activeHqCount > 0
                  ? `Live 60p All (${activeHqCount}/${eligibleHqIds.length})`
                  : "Live 60p All"}
              </span>
            </button>
          )}
          <button
            type="button"
            onClick={() => setNoteOpen((prev) => !prev)}
            aria-expanded={noteOpen}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-3 text-xs font-medium text-[var(--text-secondary)] transition-colors hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            title="Log a timestamped operator note into the Event Log and Excel report"
          >
            <BookmarkPlus size={13} strokeWidth={1.5} aria-hidden="true" />
            <span>Log Note</span>
          </button>
          <button
            type="button"
            disabled={resettingAllLufs}
            onClick={() => void handleResetAll()}
            title="Reset integrated average LUFS across all channels"
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs font-medium transition-colors disabled:opacity-50 ${
              clearedAllLufs
                ? "border-[var(--positive)]/50 bg-[var(--positive-soft)] text-[var(--positive-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            }`}
          >
            <RotateCcw
              size={13}
              strokeWidth={1.5}
              className={resettingAllLufs ? "animate-spin" : ""}
              aria-hidden="true"
            />
            <span>
              {resettingAllLufs ? "Resetting..." : clearedAllLufs ? "Loudness Cleared" : "Reset All Loudness"}
            </span>
          </button>
        </div>
      </div>

      {noteOpen && (
        <form
          onSubmit={(e) => void submitBookmark(e)}
          className="flex flex-wrap items-center gap-2 rounded-xl border border-[var(--border-strong)] bg-[var(--bg-card)] p-3"
        >
          <select
            aria-label="Note target channel"
            value={noteChannelId}
            onChange={(e) => setNoteChannelId(e.target.value)}
            className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs text-[var(--text-primary)]"
          >
            <option value="">Show-Wide Event</option>
            {snapshot.channels.map((c) => (
              <option key={c.config.id} value={c.config.id}>
                {c.config.name}
              </option>
            ))}
          </select>
          <input
            type="text"
            required
            maxLength={240}
            value={noteText}
            onChange={(e) => setNoteText(e.target.value)}
            placeholder="Operator note (e.g. 'Switched main encoder to backup', 'Sponsor segment started')"
            className="h-8 min-w-[220px] flex-1 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs text-[var(--text-primary)]"
          />
          <button
            type="submit"
            disabled={savingNote || !noteText.trim()}
            className="inline-flex h-8 items-center rounded-lg bg-[var(--text-primary)] px-3 text-xs font-medium text-[var(--bg-rail)] disabled:opacity-50"
          >
            {savingNote ? "Saving..." : "Save Note"}
          </button>
        </form>
      )}

      <GovernorStatusStrip governor={snapshot.governor} />

      {snapshot.show.groups.map((grp) => {
        const members = snapshot.channels.filter((c) => c.config.group_id === grp.id);
        const activeCount = members.filter((c) => c.config.enabled).length;
        const groupCcv = members.reduce(
          (acc, c) => acc + (c.ccv.status === "VALUE" && c.ccv.value ? c.ccv.value : 0),
          0
        );
        const groupHqIds = members.filter(canChannelUseHq).map((c) => c.config.id);
        const groupHqActive = groupHqIds.filter((id) => Boolean(hqByChannel[id])).length;
        const allGroupHq = groupHqIds.length > 0 && groupHqActive === groupHqIds.length;

        return (
          <section key={grp.id} className="space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-3 border-b border-[var(--border-hairline)] pb-2">
              <div className="flex flex-wrap items-center gap-2 sm:gap-3">
                <h2 className="text-base font-medium text-[var(--text-primary)]">{grp.name}</h2>
                <span className="rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-0.5 text-xs font-mono-tabular text-[var(--text-secondary)]">
                  {groupCcv.toLocaleString()} viewers
                </span>
                <span className="text-xs font-mono-tabular text-[var(--text-muted)]">
                  {activeCount}/{members.length} active
                </span>
                {grp.complaint_keywords.length > 0 && (
                  <span
                    className="rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2 py-0.5 text-[11px] font-mono-tabular text-[var(--text-muted)]"
                    title={`Watched chat keywords: ${grp.complaint_keywords.join(", ")}`}
                  >
                    {grp.complaint_keywords.length} chat keywords
                  </span>
                )}
              </div>

              <div className="flex flex-wrap items-center gap-2">
                {groupHqIds.length > 0 && (
                  <button
                    type="button"
                    aria-pressed={allGroupHq}
                    onClick={() => setBatchHq(groupHqIds, !allGroupHq)}
                    title={
                      allGroupHq
                        ? `Return all ${grp.name} feeds to 1.5fps thumbnails`
                        : `Switch all eligible ${grp.name} feeds to 60p HQ live video`
                    }
                    className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
                      allGroupHq
                        ? "border-[#4cc38a]/50 bg-[#4cc38a]/12 text-[#4cc38a] hover:bg-[#4cc38a]/20"
                        : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                    }`}
                  >
                    <MonitorPlay size={13} strokeWidth={1.5} aria-hidden="true" />
                    <span>
                      {allGroupHq
                        ? "Preview Group"
                        : groupHqActive > 0
                        ? `Live 60p (${groupHqActive}/${groupHqIds.length})`
                        : "Live 60p Group"}
                    </span>
                  </button>
                )}

                <button
                  type="button"
                  onClick={() => onToggleGroupIntermission(grp.id, !grp.intermission_active)}
                  aria-pressed={grp.intermission_active}
                  className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
                    grp.intermission_active
                      ? "border-[var(--warning)]/50 bg-[var(--warning-soft)] text-[var(--warning-text)]"
                      : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                  }`}
                >
                  {grp.intermission_active ? (
                    <PauseCircle size={14} strokeWidth={1.5} />
                  ) : (
                    <PlayCircle size={14} strokeWidth={1.5} />
                  )}
                  <span>{grp.intermission_active ? "Group Hold On" : "Group Hold"}</span>
                </button>
              </div>
            </div>

            <div
              className={`grid grid-cols-1 items-start gap-4 sm:grid-cols-2 ${
                members.length >= 3 ? "xl:grid-cols-3 2xl:grid-cols-4" : ""
              }`}
            >
              {members.map((ch) => (
                <ChannelCard
                  key={ch.config.id}
                  channel={ch}
                  liveThumbB64={thumbs[ch.config.id]}
                  liveAudio={meters[ch.config.id]}
                  globalPflMuted={globalPflMuted}
                  livePflMeter={snapshot.active_pfl_channel_id === ch.config.id ? pcmPflMeter : null}
                  hqEnabled={Boolean(hqByChannel[ch.config.id])}
                  onSetHqEnabled={handleSetChannelHq}
                  onMseAudioChange={onMseAudioChange}
                  onAck={onAck}
                  onToggleEnabled={onToggleChannelEnabled}
                  onForceClip={onForceClip}
                  onTogglePfl={onTogglePfl}
                  onResetLufs={onResetLufs}
                  onSetLufsTarget={onSetLufsTarget}
                  onLoginFallback={onLoginFallback}
                  onWipeAuth={onWipeAuth}
                  onRetryChannel={onRetryChannel}
                />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

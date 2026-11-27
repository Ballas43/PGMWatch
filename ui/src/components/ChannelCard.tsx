import { useEffect, useRef, useState } from "react";
import {
  Activity,
  Check,
  ChevronDown,
  ChevronUp,
  Film,
  Headphones,
  KeyRound,
  Loader2,
  Pause,
  Play,
  Trash2,
} from "lucide-react";
import { AlertState, AudioTelemetry, ChannelRuntimeSnapshot } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { StereoMeterSample } from "../utils/audioAnalyser";
import { ChannelAudioMeter, formatAvgLufs } from "./ChannelAudioMeter";
import { ChannelDiagnosticsDrawer } from "./ChannelDiagnosticsDrawer";
import { ChannelSignalStrip } from "./ChannelSignalStrip";
import { InfoTip } from "./InfoTip";
import { LiveMseVideo } from "./LiveMseVideo";

interface ChannelCardProps {
  channel: ChannelRuntimeSnapshot;
  liveThumbB64?: string;
  liveAudio?: AudioTelemetry;
  globalPflMuted?: boolean;
  livePflMeter?: StereoMeterSample | null;
  hqEnabled?: boolean;
  onSetHqEnabled?: (id: string, next: boolean) => void;
  onMseAudioChange?: (id: string, hasMseAudio: boolean) => void;
  onAck: (id: string) => void;
  onToggleEnabled: (id: string, enabled: boolean) => Promise<void> | void;
  onForceClip: (id: string) => void;
  onTogglePfl: (id: string) => void;
  onResetLufs: (id: string) => Promise<void> | void;
  onSetLufsTarget: (
    id: string,
    targetLufs: number | null,
    lufsTolerance: number | null
  ) => Promise<void> | void;
  onLoginFallback: (id: string) => void;
  onWipeAuth: (id: string) => void;
  onRetryChannel: (id: string) => void;
}

function stateStyle(state: AlertState): { dot: string; border: string; label: string } {
  switch (state) {
    case "CRITICAL":
      return { dot: "bg-[#e5484d]", border: "border-[#e5484d]/60", label: "Critical" };
    case "WARNING":
      return { dot: "bg-[#e5a93b]", border: "border-[#e5a93b]/50", label: "Warning" };
    case "ACKNOWLEDGED":
      return { dot: "bg-[#9e353a]", border: "border-[#9e353a]/60", label: "Acked" };
    case "AD_BREAK":
      return { dot: "bg-[#3b82f6]", border: "border-[#3b82f6]/50", label: "Ad Break" };
    case "LOGIN_GATED":
      return { dot: "bg-[#a78bfa]", border: "border-[#a78bfa]/50", label: "Login Gated" };
    case "EXTRACTOR_ERR":
      return { dot: "bg-[#a78bfa]", border: "border-[#a78bfa]/50", label: "Extractor Err" };
    case "REACQUIRING":
      return { dot: "bg-[#85858b]", border: "border-[var(--border-hairline)]", label: "Reacquiring" };
    case "HEALTHY":
      return { dot: "bg-[#4cc38a]", border: "border-[var(--border-hairline)]", label: "Nominal" };
    default:
      return { dot: "bg-[#85858b]", border: "border-[var(--border-hairline)]", label: "Offline" };
  }
}

function formatPlatform(p: ChannelRuntimeSnapshot["config"]["platform"]): string {
  switch (p) {
    case "youtube":
      return "YouTube";
    case "twitch":
      return "Twitch";
    case "facebook":
      return "Facebook";
    case "tiktok":
      return "TikTok";
    case "bigo":
      return "Bigo";
    default:
      return "Custom";
  }
}

export function canChannelUseHq(channel: ChannelRuntimeSnapshot): boolean {
  return (
    channel.effective_tier !== "LIGHT" &&
    Boolean(channel.resolved_via) &&
    channel.resolved_via !== "simulator" &&
    channel.config.enabled
  );
}

export function ChannelCard({
  channel,
  liveThumbB64,
  liveAudio,
  globalPflMuted = false,
  livePflMeter = null,
  hqEnabled: controlledHqEnabled,
  onSetHqEnabled,
  onMseAudioChange,
  onAck,
  onToggleEnabled,
  onForceClip,
  onTogglePfl,
  onResetLufs,
  onSetLufsTarget,
  onLoginFallback,
  onWipeAuth,
  onRetryChannel,
}: ChannelCardProps) {
  const [localHqEnabled, setLocalHqEnabled] = useState(false);
  const hqEnabled = controlledHqEnabled ?? localHqEnabled;
  const setHqEnabled = (nextOrUpdater: boolean | ((prev: boolean) => boolean)) => {
    const resolved =
      typeof nextOrUpdater === "function" ? nextOrUpdater(hqEnabled) : nextOrUpdater;
    if (onSetHqEnabled) {
      onSetHqEnabled(channel.config.id, resolved);
    } else {
      setLocalHqEnabled(resolved);
    }
  };
  const [mseActive, setMseActive] = useState(false);
  const [mseHasAudio, setMseHasAudio] = useState(false);
  const [mseFps, setMseFps] = useState<number | null>(null);
  const [mseAudioMeter, setMseAudioMeter] = useState<StereoMeterSample | null>(null);
  const [diagOpen, setDiagOpen] = useState(false);
  const [clipSaved, setClipSaved] = useState(false);
  const clipTimerRef = useRef<number | null>(null);
  const mseAudioCbRef = useRef(onMseAudioChange);
  mseAudioCbRef.current = onMseAudioChange;

  useEffect(() => {
    return () => {
      if (clipTimerRef.current !== null) {
        window.clearTimeout(clipTimerRef.current);
      }
    };
  }, []);

  const handleClipClick = () => {
    onForceClip(channel.config.id);
    setClipSaved(true);
    if (clipTimerRef.current !== null) {
      window.clearTimeout(clipTimerRef.current);
    }
    clipTimerRef.current = window.setTimeout(() => {
      setClipSaved(false);
    }, 1800);
  };

  const thumb = liveThumbB64 ?? channel.thumbnail_b64;
  const audio = liveAudio ?? channel.audio;
  const st = stateStyle(channel.alert_state);
  const isLight = channel.effective_tier === "LIGHT";
  const isVertical = channel.config.orientation_vertical;
  const isPaused = !channel.config.enabled;
  const canUseMse = canChannelUseHq(channel);
  const showMse = canUseMse && hqEnabled;

  useEffect(() => {
    if (!showMse) {
      setMseActive(false);
      setMseHasAudio(false);
      setMseFps(null);
      setMseAudioMeter(null);
      mseAudioCbRef.current?.(channel.config.id, false);
    }
  }, [showMse, channel.config.id]);

  const useMseAudio = showMse && mseActive && mseHasAudio;
  const activePflMeter =
    channel.pfl_active && !globalPflMuted
      ? useMseAudio
        ? mseAudioMeter
        : livePflMeter
      : null;
  const pflSyncMode = activePflMeter ? (useMseAudio ? "hq-av" : "pcm-audio") : null;
  const meterAudio = activePflMeter
    ? { ...audio, left_rms_dbfs: activePflMeter.leftDb, right_rms_dbfs: activePflMeter.rightDb }
    : audio;

  useEffect(() => {
    const channelId = channel.config.id;
    return () => {
      mseAudioCbRef.current?.(channelId, false);
    };
  }, [channel.config.id]);

  const mediaClass = isVertical
    ? "mx-auto h-full w-auto aspect-[9/16] object-cover"
    : "h-full w-full object-cover";
  const diagPanelId = `diag-${channel.config.id}`;

  return (
    <article
      className={`flex flex-col rounded-xl border bg-[var(--bg-card)] p-2 transition-colors ${st.border} ${
        isPaused ? "opacity-75" : ""
      }`}
    >
      {/* Header row */}
      <div className="mb-1.5 flex items-start justify-between gap-2">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-medium text-[var(--text-primary)]">
            {channel.config.name}
          </h3>
          <div className="mt-0.5 flex flex-wrap items-center gap-2 text-xs text-[var(--text-muted)]">
            <span>{formatPlatform(channel.config.platform)}</span>
            <InfoTip entry={ANALYTICS_GLOSSARY.tier} underline focusable>
              <span className="font-mono-tabular">{channel.effective_tier}</span>
            </InfoTip>
            {isPaused && (
              <span className="inline-flex items-center gap-1 rounded border border-[var(--border-strong)] bg-[var(--bg-raised)] px-1.5 py-0.5 text-[10px] font-mono-tabular text-[var(--text-secondary)]">
                <Pause size={10} strokeWidth={1.5} aria-hidden="true" />
                <span>PAUSED</span>
              </span>
            )}
            {channel.video_shed_resource && (
              <InfoTip entry={ANALYTICS_GLOSSARY.shedChannels} focusable>
                <span className="rounded bg-[#e5a93b]/15 px-1.5 py-0.5 text-[11px] font-mono-tabular text-[#e5a93b]">
                  VIDEO_SHED_RESOURCE
                </span>
              </InfoTip>
            )}
          </div>
        </div>

        <div className="text-right font-mono-tabular shrink-0">
          <InfoTip entry={ANALYTICS_GLOSSARY.ccv} underline focusable>
            <div className="text-xs font-medium text-[var(--text-primary)]">
              {channel.ccv.status === "VALUE" && channel.ccv.value !== null
                ? `${channel.ccv.value.toLocaleString()} viewers`
                : channel.ccv.status === "UNAVAILABLE"
                ? "Viewers N/A"
                : "Not Tracked"}
            </div>
          </InfoTip>
          <InfoTip entry={ANALYTICS_GLOSSARY.uptime} focusable>
            <div className="text-[11px] text-[var(--text-muted)]">
              {channel.uptime_pct.toFixed(1)}% up
            </div>
          </InfoTip>
        </div>
      </div>

      {/* Video Monitor Canvas — uniform 16:9 multiviewer frame with centered 9:16 pillarbox */}
      <div className="group/mon relative aspect-video w-full overflow-hidden rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-rail)]">
        {isLight ? (
          <div className="flex h-full w-full flex-col items-center justify-center px-3 text-center">
            <span className="font-mono-tabular text-xs text-[var(--text-secondary)]">
              Audio-Only Monitor
            </span>
            <span className="mt-1 font-mono-tabular text-[11px] text-[var(--text-muted)]">
              Video decode paused (0% GPU)
            </span>
          </div>
        ) : (
          <>
            {(!showMse || !mseActive) &&
              (thumb ? (
                <img
                  src={`data:image/jpeg;base64,${thumb}`}
                  alt={`${channel.config.name} live preview`}
                  className={mediaClass}
                />
              ) : (
                <div className="flex h-full w-full items-center justify-center text-xs font-mono-tabular text-[var(--text-muted)]">
                  {isPaused ? "Feed Paused" : "Awaiting frame"}
                </div>
              ))}
            {showMse && (
              <LiveMseVideo
                channelId={channel.config.id}
                streamKey={`${channel.resolved_via ?? ""}:${channel.effective_tier}:${channel.config.url}`}
                className={`pointer-events-none absolute inset-0 ${mediaClass}`}
                unmuted={channel.pfl_active && !globalPflMuted}
                onStatusChange={(active, fps, hasAudio) => {
                  const audioActive = active && hasAudio;
                  setMseActive((prev) => (prev === active ? prev : active));
                  setMseHasAudio((prev) => (prev === audioActive ? prev : audioActive));
                  if (fps !== null) setMseFps((prev) => (prev === fps ? prev : fps));
                  if (!active) setMseFps(null);
                  mseAudioCbRef.current?.(channel.config.id, audioActive);
                }}
                onAudioMeter={setMseAudioMeter}
              />
            )}
            {canUseMse && (
              <button
                type="button"
                onClick={() => setHqEnabled((prev) => !prev)}
                aria-pressed={hqEnabled}
                aria-label={
                  hqEnabled
                    ? `Return ${channel.config.name} preview to 1.5fps thumbnail`
                    : `Load live 60p HQ preview for ${channel.config.name}`
                }
                title={
                  hqEnabled
                    ? "Live 60p HQ stream active (browser locks audio/video sync). Click to return to 1.5fps thumbnail."
                    : "Low-overhead 1.5fps thumbnail. Click to stream live 60p HQ (A/V synced)."
                }
                className="absolute inset-0 z-10 cursor-pointer bg-transparent focus:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[#4cc38a]"
              />
            )}
          </>
        )}

        {/* Signature: Broadcast Tally Dot + Tabular LUFS/Status Pill (over video canvas — stays high-contrast dark) */}
        <div className="pointer-events-none absolute left-2 top-2 z-20 inline-flex max-w-[calc(100%-1rem)] items-center gap-1.5 rounded border border-white/15 bg-[#0f0f10]/85 px-2 py-0.5 text-[11px] font-mono-tabular text-[#eef0f4] whitespace-nowrap">
          <span className={`h-2 w-2 shrink-0 rounded-full ${st.dot}`} aria-hidden="true" />
          <span>{st.label}</span>
          <span className="text-[#9ca3af]">·</span>
          <span>{formatAvgLufs(audio.integrated_lufs)} LUFS</span>
          {(channel.config.broadcast_delay_secs ?? 0) > 0 && (
            <>
              <span className="text-[#9ca3af]">·</span>
              <span className="text-[#93c5fd]">
                +{Math.round((channel.config.broadcast_delay_secs ?? 0) / 60)}m delay
              </span>
            </>
          )}
          {channel.config.exclude_from_accounting && (
            <>
              <span className="text-[#9ca3af]">·</span>
              <span className="text-[#9ca3af]">Excluded</span>
            </>
          )}
          {channel.config.live_streams_only === false && (
            <>
              <span className="text-[#9ca3af]">·</span>
              <span className="text-[#f3c677]">VOD Test</span>
            </>
          )}
          {showMse && mseActive ? (
            <>
              <span className="text-[#9ca3af]">·</span>
              <span className="text-[#4cc38a]">{mseFps ? `${mseFps}p Live` : "60p Live"}</span>
            </>
          ) : canUseMse ? (
            <>
              <span className="text-[#9ca3af]">·</span>
              <span className={showMse ? "text-[#f3c677]" : "text-[#a1a1aa]"}>
                {showMse ? "60p…" : "1.5fps"}
              </span>
            </>
          ) : null}
        </div>

        {channel.alert_state === "AD_BREAK" && (
          <div className="pointer-events-none absolute right-2 top-2 z-20 rounded border border-[#3b82f6]/50 bg-[#0f0f10]/90 px-2 py-0.5 text-[10px] font-mono-tabular font-medium text-[#93c5fd]">
            Ad Break
          </div>
        )}

        {canUseMse && (
          <div
            aria-hidden="true"
            className={`pointer-events-none absolute bottom-2 right-2 z-20 inline-flex items-center gap-1.5 rounded border px-2 py-0.5 text-[10px] font-mono-tabular font-medium transition-[opacity,border-color,color] ${
              showMse && mseActive
                ? "border-[#4cc38a]/50 bg-[#0f0f10]/80 text-[#4cc38a] opacity-80 group-hover/mon:opacity-100"
                : showMse
                ? "border-[#e5a93b]/55 bg-[#0f0f10]/90 text-[#f3c677]"
                : "border-white/25 bg-[#0f0f10]/85 text-[#eef0f4] group-hover/mon:border-[#4cc38a]/70 group-hover/mon:text-[#4cc38a]"
            }`}
          >
            {showMse && mseActive ? (
              <>
                <span className="h-1.5 w-1.5 rounded-full bg-[#4cc38a]" />
                <span className="group-hover/mon:hidden">
                  LIVE {mseFps ? `${mseFps}p` : "60p"}
                </span>
                <span className="hidden group-hover/mon:inline">
                  Live 60p · Click for Preview
                </span>
              </>
            ) : showMse ? (
              <>
                <Loader2 size={10} strokeWidth={2} className="animate-spin" />
                <span>Loading 60p… · Cancel</span>
              </>
            ) : (
              <>
                <Play size={10} strokeWidth={2} />
                <span>Click for Live 60p</span>
              </>
            )}
          </div>
        )}
      </div>

      {/* Audio VU + Telemetry */}
      <ChannelAudioMeter
        channelId={channel.config.id}
        channelName={channel.config.name}
        audio={meterAudio}
        pflSyncMode={pflSyncMode}
        onSwitchToHq={
          pflSyncMode === "pcm-audio" && canUseMse && !showMse ? () => setHqEnabled(true) : undefined
        }
        targetLufs={
          channel.effective_target_lufs ??
          channel.config.target_lufs ??
          (channel.config.platform === "tiktok" || channel.config.platform === "bigo"
            ? -11
            : channel.config.platform === "facebook"
            ? -13
            : -14)
        }
        lufsTolerance={channel.effective_lufs_tolerance ?? channel.config.lufs_tolerance ?? 2.5}
        isCustomTarget={channel.config.target_lufs != null}
        onResetLufs={onResetLufs}
        onSetLufsTarget={onSetLufsTarget}
      />

      {/* Video Decode, Social & Simultaneous Fault Telemetry */}
      <ChannelSignalStrip channel={channel} />

      {channel.last_status_reason && (
        <div className="mt-1.5 truncate rounded bg-[var(--bg-raised)] px-2 py-1 text-xs text-[var(--warning-text)]" title={channel.last_status_reason}>
          {channel.last_status_reason}
        </div>
      )}

      {/* Action Strip */}
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        <button
          type="button"
          onClick={() => onTogglePfl(channel.config.id)}
          aria-pressed={channel.pfl_active}
          title={
            channel.pfl_active
              ? "Listening to this channel's live audio (PFL). Click to stop."
              : "Listen to this channel's live audio (Pre-Fader Listen)"
          }
          className={`inline-flex h-8 flex-1 items-center justify-center gap-1.5 rounded-lg border px-2 text-xs font-medium transition-colors ${
            channel.pfl_active
              ? "border-[var(--accent)]/50 bg-[var(--accent-soft)] text-[var(--accent-text)]"
              : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
          }`}
        >
          <Headphones size={14} strokeWidth={1.5} />
          <span>{channel.pfl_active ? "Listening" : "Listen"}</span>
        </button>

        <InfoTip entry={ANALYTICS_GLOSSARY.forceClip}>
          <button
            type="button"
            onClick={handleClipClick}
            aria-label={`Save instant pre-trigger replay clip for ${channel.config.name}`}
            className={`inline-flex h-8 items-center justify-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
              clipSaved
                ? "border-[var(--positive)]/50 bg-[var(--positive-soft)] text-[var(--positive-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            }`}
          >
            {clipSaved ? <Check size={14} strokeWidth={1.5} /> : <Film size={14} strokeWidth={1.5} />}
            <span>{clipSaved ? "Saved" : "Clip"}</span>
          </button>
        </InfoTip>

        <button
          type="button"
          onClick={() => setDiagOpen((prev) => !prev)}
          aria-expanded={diagOpen}
          aria-controls={diagPanelId}
          aria-label={`Toggle detailed metrics for ${channel.config.name}`}
          className={`inline-flex h-8 items-center justify-center gap-1 rounded-lg border px-2 text-xs font-medium transition-colors ${
            diagOpen
              ? "border-[var(--border-strong)] bg-[var(--bg-page)] text-[var(--text-primary)]"
              : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
          }`}
          title="Inspect detailed video, loudness, audio hum, and viewer source metrics"
        >
          <Activity size={13} strokeWidth={1.5} aria-hidden="true" />
          <span>Details</span>
          {diagOpen ? <ChevronUp size={12} strokeWidth={1.5} /> : <ChevronDown size={12} strokeWidth={1.5} />}
        </button>

        <button
          type="button"
          onClick={() => void onToggleEnabled(channel.config.id, !channel.config.enabled)}
          aria-pressed={!channel.config.enabled}
          aria-label={
            channel.config.enabled
              ? `Pause monitoring for ${channel.config.name}`
              : `Resume monitoring for ${channel.config.name}`
          }
          title={channel.config.enabled ? "Pause monitoring this feed" : "Resume monitoring this feed"}
          className={`inline-flex h-8 w-8 items-center justify-center rounded-lg border transition-colors ${
            channel.config.enabled
              ? "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              : "border-[var(--positive)]/50 bg-[var(--positive-soft)] text-[var(--positive-text)] hover:bg-[var(--positive)]/25"
          }`}
        >
          {channel.config.enabled ? <Pause size={13} strokeWidth={1.5} /> : <Play size={13} strokeWidth={1.5} />}
        </button>

        {(channel.alert_state === "WARNING" || channel.alert_state === "CRITICAL") && (
          <button
            type="button"
            onClick={() => onAck(channel.config.id)}
            className="inline-flex h-8 items-center justify-center gap-1 rounded-lg bg-[#e5484d] px-2.5 text-xs font-medium text-white transition-opacity hover:opacity-90"
          >
            <Check size={14} strokeWidth={1.5} />
            <span>Acknowledge</span>
          </button>
        )}

        {channel.alert_state === "LOGIN_GATED" && (
          <>
            <button
              type="button"
              onClick={() => onLoginFallback(channel.config.id)}
              className="inline-flex h-8 items-center justify-center gap-1.5 rounded-lg border border-[var(--auth)]/40 bg-[var(--auth)]/15 px-2.5 text-xs font-medium text-[var(--auth-text)] hover:bg-[var(--auth)]/25"
              title="Open isolated Edge profile to sign in manually"
            >
              <KeyRound size={14} strokeWidth={1.5} />
              <span>Sign In</span>
            </button>
            <button
              type="button"
              onClick={() => onRetryChannel(channel.config.id)}
              className="inline-flex h-8 items-center justify-center rounded-lg border border-[var(--positive)]/45 bg-[var(--positive-soft)] px-2.5 text-xs font-medium text-[var(--positive-text)] hover:bg-[var(--positive)]/25"
              title="Close the Edge sign-in window first, then click Retry"
            >
              Retry
            </button>
            <button
              type="button"
              onClick={() => onWipeAuth(channel.config.id)}
              aria-label="Wipe burner auth profile"
              title="Wipe isolated browser auth profile"
              className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] hover:text-[#e5484d]"
            >
              <Trash2 size={14} strokeWidth={1.5} />
            </button>
          </>
        )}

        {(channel.alert_state === "EXTRACTOR_ERR" || channel.alert_state === "OFFLINE") && (
          <button
            type="button"
            onClick={() => onRetryChannel(channel.config.id)}
            className="inline-flex h-8 items-center justify-center rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs font-medium text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
          >
            Retry
          </button>
        )}
      </div>

      {channel.alert_state === "LOGIN_GATED" && (
        <p className="mt-1.5 text-[11px] text-[var(--text-muted)]">
          Click Sign In, log in inside the opened Edge window, close it, then click Retry.
        </p>
      )}

      {diagOpen && (
        <ChannelDiagnosticsDrawer id={diagPanelId} channel={channel} audio={audio} />
      )}
    </article>
  );
}

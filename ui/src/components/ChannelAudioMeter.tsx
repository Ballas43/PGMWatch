import { useEffect, useRef, useState } from "react";
import { RotateCcw } from "lucide-react";
import { AudioTelemetry } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { InfoTip } from "./InfoTip";
import { LufsTargetEditor } from "./LufsTargetEditor";

interface ChannelAudioMeterProps {
  channelId: string;
  channelName: string;
  audio: AudioTelemetry;
  pflSyncMode?: "hq-av" | "pcm-audio" | null;
  onSwitchToHq?: () => void;
  targetLufs: number;
  lufsTolerance: number;
  isCustomTarget: boolean;
  onResetLufs: (id: string) => Promise<void> | void;
  onSetLufsTarget: (
    id: string,
    targetLufs: number | null,
    lufsTolerance: number | null
  ) => Promise<void> | void;
}

export function formatAvgLufs(lufs: number): string {
  if (!Number.isFinite(lufs) || lufs <= -69.9) {
    return "--.-";
  }
  return lufs.toFixed(1);
}

function dbToPct(db: number): number {
  return Math.max(0, Math.min(100, ((db + 60) / 60) * 100));
}

function vuBarColor(dbfs: number): string {
  if (dbfs >= -1.0) return "bg-[#e5484d]";
  if (dbfs >= -9.0) return "bg-[#e5a93b]";
  return "bg-[#4cc38a]";
}

export function ChannelAudioMeter({
  channelId,
  channelName,
  audio,
  pflSyncMode = null,
  onSwitchToHq,
  targetLufs,
  lufsTolerance,
  isCustomTarget,
  onResetLufs,
  onSetLufsTarget,
}: ChannelAudioMeterProps) {
  const [resetting, setResetting] = useState(false);
  const [cleared, setCleared] = useState(false);
  const timerRef = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
    };
  }, []);

  const handleReset = async () => {
    if (resetting) return;
    setResetting(true);
    try {
      await onResetLufs(channelId);
      setCleared(true);
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
      }
      timerRef.current = window.setTimeout(() => setCleared(false), 1200);
    } finally {
      setResetting(false);
    }
  };

  const avgText = formatAvgLufs(audio.integrated_lufs);
  const shortTermText = formatAvgLufs(audio.short_term_lufs);

  return (
    <div className="mt-1.5 space-y-1 font-mono-tabular text-[11px]">
      <div className="flex flex-wrap items-center justify-between gap-1.5 pb-0.5">
        <div className="flex flex-wrap items-center gap-2">
          <InfoTip entry={ANALYTICS_GLOSSARY.avgLufs} underline focusable>
            <span className="text-[var(--text-secondary)]">
              <span className="text-[var(--text-muted)]">Avg</span>{" "}
              <span className="font-medium text-[var(--text-primary)]">{avgText}</span>
            </span>
          </InfoTip>
          <span
            className="text-[var(--text-secondary)]"
            title="3-second Short-Term LUFS (live loudness dynamics; incident alerting uses Avg LUFS)"
          >
            <span className="text-[var(--text-muted)]">ST</span>{" "}
            <span className="font-medium text-[var(--text-primary)]">{shortTermText}</span>{" "}
            <span className="text-[var(--text-muted)]">LUFS</span>
          </span>
          {pflSyncMode && (
            <span
              className="inline-flex items-center gap-1 rounded border border-[#4cc38a]/35 bg-[#4cc38a]/10 px-1.5 py-0.5 text-[10px] font-medium text-[#4cc38a]"
              title={
                pflSyncMode === "hq-av"
                  ? "Live 60p video, headphone audio, and L/R meters are clock-locked at 0ms."
                  : "L/R bars are driven directly by live headphone playout (0ms audio-to-meter lock)."
              }
            >
              0ms Audio Sync
            </span>
          )}
          {onSwitchToHq && (
            <button
              type="button"
              onClick={onSwitchToHq}
              title="Enable Live 60p video on this channel so video also locks 0ms with audio & meters"
              className="rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--text-secondary)] transition-colors hover:border-[#4cc38a]/60 hover:text-[#4cc38a]"
            >
              Sync Video (60p)
            </button>
          )}
        </div>
        <div className="flex items-center gap-1">
          <LufsTargetEditor
            channelId={channelId}
            channelName={channelName}
            effectiveTarget={targetLufs}
            effectiveTolerance={lufsTolerance}
            isCustomOverride={isCustomTarget}
            onSetLufsTarget={onSetLufsTarget}
          />
          <button
            type="button"
            disabled={resetting}
            onClick={() => void handleReset()}
            aria-label={`Reset all audio metrics for ${channelName}`}
            title="Reset all audio metrics (Average LUFS, Short-Term LUFS, True Peak, and fault timers)"
            className={`inline-flex h-6 items-center gap-1 rounded border px-2 text-[11px] font-medium transition-colors disabled:opacity-50 ${
              cleared
                ? "border-[var(--positive)]/50 bg-[var(--positive-soft)] text-[var(--positive-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            }`}
          >
            <RotateCcw
              size={11}
              strokeWidth={1.5}
              className={resetting ? "animate-spin" : ""}
              aria-hidden="true"
            />
            <span>{resetting ? "Resetting..." : cleared ? "Cleared" : "Reset"}</span>
          </button>
        </div>
      </div>

      <div className="flex items-center gap-2">
        <InfoTip entry={ANALYTICS_GLOSSARY.vuBars} focusable className="w-3 shrink-0 text-[var(--text-muted)]">
          L
        </InfoTip>
        <div className="h-1.5 flex-1 overflow-hidden rounded-sm bg-[var(--bg-raised)]">
          <div
            className={`h-full ${pflSyncMode ? "transition-none" : "transition-[width] duration-75"} ${vuBarColor(audio.left_rms_dbfs)}`}
            style={{ width: `${dbToPct(audio.left_rms_dbfs)}%` }}
          />
        </div>
        <InfoTip
          entry={ANALYTICS_GLOSSARY.truePeak}
          underline
          focusable
          className="w-24 shrink-0 whitespace-nowrap text-right text-[var(--text-secondary)]"
        >
          {audio.true_peak_dbtp.toFixed(1)} dBTP
        </InfoTip>
      </div>

      <div className="flex items-center gap-2">
        <InfoTip entry={ANALYTICS_GLOSSARY.vuBars} focusable className="w-3 shrink-0 text-[var(--text-muted)]">
          R
        </InfoTip>
        <div className="h-1.5 flex-1 overflow-hidden rounded-sm bg-[var(--bg-raised)]">
          <div
            className={`h-full ${pflSyncMode ? "transition-none" : "transition-[width] duration-75"} ${vuBarColor(audio.right_rms_dbfs)}`}
            style={{ width: `${dbToPct(audio.right_rms_dbfs)}%` }}
          />
        </div>
        <InfoTip
          entry={ANALYTICS_GLOSSARY.phaseCorr}
          underline
          focusable
          className="w-24 shrink-0 whitespace-nowrap text-right text-[var(--text-muted)]"
        >
          Phase {audio.phase_correlation.toFixed(2)}
        </InfoTip>
      </div>

      {audio.hum_freq_hz !== null && audio.hum_peak_db > 20.0 && (
        <InfoTip entry={ANALYTICS_GLOSSARY.mainsHum} focusable className="block">
          <div className="flex items-center justify-between gap-2 rounded border border-[var(--warning)]/40 bg-[var(--warning-soft)] px-2 py-0.5 text-[11px] text-[var(--warning-text)]">
            <span>Mains Hum Detected</span>
            <span>
              {Math.round(audio.hum_freq_hz)} Hz · {audio.hum_peak_db.toFixed(1)} dB
            </span>
          </div>
        </InfoTip>
      )}
    </div>
  );
}

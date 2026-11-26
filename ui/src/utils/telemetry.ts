import type { FaultKind, TriStateMetric } from "../types";

export const FAULT_LABELS: Record<FaultKind, string> = {
  STREAM_STALL: "Stream Stall",
  CDN_STARVATION: "CDN Starvation",
  BITRATE_COLLAPSE: "Bitrate Collapse",
  PTS_DRIFT: "PTS Discontinuity",
  AV_SYNC_DRIFT: "A/V Sync Drift",
  CORRUPT_DECODE: "Corrupt Decode",
  LUFS_OUT_OF_RANGE: "LUFS Out of Spec",
  AUDIO_CLIPPING: "True Peak Clipping",
  AUDIO_MICRO_SILENCE: "Audio Micro-Silence",
  AUDIO_DEAD_AIR: "Audio Dead Air",
  AUDIO_MISSING_CHANNEL: "Missing L/R Channel",
  AUDIO_PHASE_INVERSION: "Phase Inversion",
  AUDIO_MAINS_HUM: "Mains Hum / Feedback",
  VIDEO_BLACK: "Black Frame",
  VIDEO_BLINK: "Video Blink",
  VIDEO_FROZEN: "Frozen Frame",
  VIDEO_MACROBLOCKING: "Macroblocking",
  WRONG_BUS_AUDIO: "Wrong-Bus Audio",
  CHAT_COMPLAINT_SPIKE: "Chat Complaint Spike",
  WORKER_GPU_FAULT: "GPU Decode Fault",
  OPERATOR_BOOKMARK: "Operator Bookmark",
};

const CRITICAL_FAULTS = new Set<FaultKind>([
  "STREAM_STALL",
  "CORRUPT_DECODE",
  "AUDIO_DEAD_AIR",
  "VIDEO_BLACK",
  "VIDEO_FROZEN",
  "WORKER_GPU_FAULT",
]);

export function formatFaultLabel(fault: FaultKind | string): string {
  return (FAULT_LABELS as Record<string, string>)[fault] ?? fault;
}

export function isCriticalFault(fault: FaultKind): boolean {
  return CRITICAL_FAULTS.has(fault);
}

export function isBookmarkFault(fault: FaultKind): boolean {
  return fault === "OPERATOR_BOOKMARK";
}

export function formatCompactNumber(value: number): string {
  if (!Number.isFinite(value)) return "0";
  const abs = Math.abs(value);
  if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (abs >= 10_000) return `${(value / 1_000).toFixed(1)}K`;
  return Math.round(value).toLocaleString();
}

export function formatBitrate(kbps: number): string {
  if (!Number.isFinite(kbps) || kbps <= 0) return "0 kbps";
  if (kbps >= 1000) return `${(kbps / 1000).toFixed(1)} Mb/s`;
  return `${Math.round(kbps)} kbps`;
}

export function formatTriStateCompact(metric: TriStateMetric): string {
  if (metric.status === "VALUE" && metric.value !== null) {
    return formatCompactNumber(metric.value);
  }
  return metric.status === "UNAVAILABLE" ? "UNAVAIL" : "N/A";
}

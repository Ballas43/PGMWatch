import {
  ChannelRuntimeSnapshot,
  DashboardSnapshot,
  GridPoint,
  LanguageGroupConfig,
} from "../../types";

export const SERIES_PALETTE = [
  "#4cc38a",
  "#e5a93b",
  "#a78bfa",
  "#ec4899",
  "#06b6d4",
  "#f97316",
  "#84cc16",
  "#38bdf8",
  "#fb7185",
  "#2dd4bf",
  "#facc15",
  "#818cf8",
  "#34d399",
  "#f472b6",
  "#fb923c",
  "#60a5fa",
];

export type ChartMode = "category" | "top_streams" | "all_streams" | "combined";

export interface ChartSeries {
  id: string;
  label: string;
  color: string;
  peak: number;
  getValue: (pt: GridPoint) => number;
}

export interface MatchBand {
  id: string;
  label: string;
  startIdx: number;
  endIdx: number;
  x1: number;
  x2: number;
  width: number;
}

export function niceAxisMax(rawMax: number): number {
  if (rawMax <= 100) return 100;
  const exp = Math.pow(10, Math.floor(Math.log10(rawMax)));
  const frac = rawMax / exp;
  const niceFrac =
    frac <= 1.2
      ? 1.2
      : frac <= 1.5
      ? 1.5
      : frac <= 2
      ? 2
      : frac <= 2.5
      ? 2.5
      : frac <= 3
      ? 3
      : frac <= 4
      ? 4
      : frac <= 5
      ? 5
      : frac <= 6
      ? 6
      : frac <= 8
      ? 8
      : 10;
  return Math.ceil((niceFrac * exp) / 4) * 4;
}

export function formatCompactAxis(v: number): string {
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
  if (v >= 1_000) {
    const k = v / 1_000;
    return Number.isInteger(k) ? `${k}k` : `${k.toFixed(1)}k`;
  }
  return String(v);
}

export function computeXTickIndices(length: number): number[] {
  if (length <= 1) return [0];
  const target = Math.min(6, length);
  const out: number[] = [];
  for (let k = 0; k < target; k++) {
    out.push(Math.round((k * (length - 1)) / (target - 1)));
  }
  return Array.from(new Set(out));
}

export function buildChartSeries(
  timeline: GridPoint[],
  channels: ChannelRuntimeSnapshot[],
  groups: LanguageGroupConfig[]
): {
  channelSeries: ChartSeries[];
  categorySeries: ChartSeries[];
  peakIdx: number;
} {
  const groupCodeById = new Map<string, string>();
  for (const g of groups) {
    groupCodeById.set(g.id, g.language_code || g.name);
  }

  const nameCounts = new Map<string, number>();
  for (const ch of channels) {
    nameCounts.set(ch.config.name, (nameCounts.get(ch.config.name) ?? 0) + 1);
  }

  const chMap = new Map<string, { id: string; label: string; groupId: string; peak: number }>();
  for (const ch of channels) {
    const dup = (nameCounts.get(ch.config.name) ?? 0) > 1;
    const gCode = groupCodeById.get(ch.config.group_id);
    const label = dup && gCode ? `${ch.config.name} (${gCode})` : ch.config.name;
    chMap.set(ch.config.id, {
      id: ch.config.id,
      label,
      groupId: ch.config.group_id,
      peak: 0,
    });
  }

  let bestPeakIdx = 0;
  let bestPeakVal = -1;
  for (let i = 0; i < timeline.length; i++) {
    const pt = timeline[i];
    if (pt.combined_ccv > bestPeakVal) {
      bestPeakVal = pt.combined_ccv;
      bestPeakIdx = i;
    }
    for (const [cid, val] of Object.entries(pt.per_channel_ccv ?? {})) {
      const existing = chMap.get(cid);
      if (existing) {
        if (val > existing.peak) existing.peak = val;
      } else {
        chMap.set(cid, { id: cid, label: cid, groupId: "__other", peak: val });
      }
    }
  }

  const channelSeries: ChartSeries[] = Array.from(chMap.values())
    .sort((a, b) => b.peak - a.peak)
    .map((item, idx) => ({
      id: item.id,
      label: item.label,
      color: SERIES_PALETTE[idx % SERIES_PALETTE.length],
      peak: item.peak,
      getValue: (pt: GridPoint) => pt.per_channel_ccv?.[item.id] ?? 0,
    }));

  const chIdsByGroup = new Map<string, string[]>();
  for (const item of chMap.values()) {
    const list = chIdsByGroup.get(item.groupId) ?? [];
    list.push(item.id);
    chIdsByGroup.set(item.groupId, list);
  }

  const categorySeries: ChartSeries[] = groups
    .map((g, idx) => {
      const memberIds = chIdsByGroup.get(g.id) ?? [];
      let peak = 0;
      for (const pt of timeline) {
        let sum = 0;
        for (const cid of memberIds) sum += pt.per_channel_ccv?.[cid] ?? 0;
        if (sum > peak) peak = sum;
      }
      return {
        id: `grp:${g.id}`,
        label: `${g.language_code} — ${g.name}`,
        color: SERIES_PALETTE[idx % SERIES_PALETTE.length],
        peak,
        getValue: (pt: GridPoint) => {
          let sum = 0;
          for (const cid of memberIds) sum += pt.per_channel_ccv?.[cid] ?? 0;
          return sum;
        },
      };
    })
    .sort((a, b) => b.peak - a.peak);

  return { channelSeries, categorySeries, peakIdx: bestPeakIdx };
}

export function computeMatchBands(
  timeline: GridPoint[],
  matches: DashboardSnapshot["matches"],
  xFor: (i: number) => number
): MatchBand[] {
  if (timeline.length === 0) return [];
  const times = timeline.map((p) => new Date(p.bucket_utc).getTime());
  return matches
    .map((m) => {
      const startMs = new Date(m.started_at).getTime();
      const endMs = m.ended_at ? new Date(m.ended_at).getTime() : Date.now();
      if (endMs < times[0]) return null;
      const startIdx = times.findIndex((t) => t >= startMs);
      if (startIdx < 0 || times[startIdx] > endMs) return null;
      let endIdx = startIdx;
      for (let idx = times.length - 1; idx >= startIdx; idx--) {
        if (times[idx] <= endMs) {
          endIdx = idx;
          break;
        }
      }
      const x1 = xFor(startIdx);
      const x2 = Math.max(x1 + 4, xFor(endIdx));
      return { id: m.id, label: m.label, startIdx, endIdx, x1, x2, width: x2 - x1 };
    })
    .filter((b): b is MatchBand => b !== null);
}

import { useMemo, useState } from "react";
import { ChannelRuntimeSnapshot, DashboardSnapshot, GridPoint } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { formatLocalHm } from "../utils/time";
import { InfoTip } from "./InfoTip";

const STREAM_COLORS = [
  "#4cc38a",
  "#e5a93b",
  "#a78bfa",
  "#ec4899",
  "#06b6d4",
  "#f97316",
  "#84cc16",
  "#38bdf8",
];

type ChartMode = "streams_total" | "streams" | "combined";

export function CcvTimelineChart({
  timeline,
  matches,
  channels = [],
}: {
  timeline: GridPoint[];
  matches: DashboardSnapshot["matches"];
  channels?: ChannelRuntimeSnapshot[];
}) {
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [mode, setMode] = useState<ChartMode>("streams_total");
  const [hiddenStreams, setHiddenStreams] = useState<Record<string, boolean>>({});

  const w = 760;
  const h = 280;
  const pad = { top: 20, right: 20, bottom: 32, left: 56 };
  const plotW = w - pad.left - pad.right;
  const plotH = h - pad.top - pad.bottom;

  // Build ordered stream series metadata from snapshot channels + any historical channel keys in timeline
  const series = useMemo(() => {
    const map = new Map<string, { id: string; label: string; color: string }>();
    channels.forEach((ch, idx) => {
      map.set(ch.config.id, {
        id: ch.config.id,
        label: ch.config.name,
        color: STREAM_COLORS[idx % STREAM_COLORS.length],
      });
    });
    for (const pt of timeline) {
      for (const id of Object.keys(pt.per_channel_ccv ?? {})) {
        if (!map.has(id)) {
          map.set(id, {
            id,
            label: id,
            color: STREAM_COLORS[map.size % STREAM_COLORS.length],
          });
        }
      }
    }
    return Array.from(map.values());
  }, [channels, timeline]);

  if (timeline.length === 0) {
    return (
      <div className="flex h-[280px] items-center justify-center text-sm text-[var(--text-muted)]">
        Waiting for first 60-second accounting interval
      </div>
    );
  }

  const visibleSeries = series.filter((s) => !hiddenStreams[s.id]);
  const showCombined = mode === "combined" || mode === "streams_total";
  const showStreams = mode === "streams" || mode === "streams_total";

  let maxVal = 100;
  for (const pt of timeline) {
    if (showCombined && pt.combined_ccv > maxVal) {
      maxVal = pt.combined_ccv;
    }
    if (showStreams) {
      for (const s of visibleSeries) {
        const v = pt.per_channel_ccv?.[s.id] ?? 0;
        if (v > maxVal) maxVal = v;
      }
    }
  }
  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((t) => Math.round(maxVal * t));

  const xFor = (i: number) =>
    pad.left + (timeline.length === 1 ? plotW / 2 : (i / (timeline.length - 1)) * plotW);
  const yFor = (val: number) => pad.top + plotH - (val / maxVal) * plotH;

  const combinedPath = timeline
    .map((p, i) => `${i === 0 ? "M" : "L"}${xFor(i).toFixed(1)},${yFor(p.combined_ccv).toFixed(1)}`)
    .join(" ");

  const combinedAreaPath = `${combinedPath} L${xFor(timeline.length - 1).toFixed(1)},${(pad.top + plotH).toFixed(1)} L${xFor(0).toFixed(1)},${(pad.top + plotH).toFixed(1)} Z`;

  const hoverPoint = hoverIdx !== null ? timeline[hoverIdx] : null;

  const toggleStream = (id: string) => {
    setHiddenStreams((prev) => ({ ...prev, [id]: !prev[id] }));
  };

  return (
    <div className="relative space-y-2.5">
      {/* Mode + Stream Legend Bar */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px]">
        <div className="inline-flex rounded-md border border-[var(--border-hairline)] bg-[var(--bg-raised)] p-0.5 font-mono-tabular">
          {(
            [
              ["streams_total", "Streams + Total"],
              ["streams", "By Stream"],
              ["combined", "Total Only"],
            ] as const
          ).map(([val, label]) => (
            <button
              key={val}
              type="button"
              onClick={() => setMode(val)}
              className={`rounded px-2 py-0.5 transition-colors ${
                mode === val
                  ? "bg-[var(--bg-card)] text-[var(--text-primary)]"
                  : "text-[var(--text-muted)] hover:text-[var(--text-secondary)]"
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        {showStreams && series.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5">
            {showCombined && (
              <InfoTip entry={ANALYTICS_GLOSSARY.ccv} focusable>
                <span className="inline-flex items-center gap-1.5 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-0.5 text-[var(--text-secondary)]">
                  <span className="inline-block h-0.5 w-3 bg-[#3b82f6]" />
                  <span>Combined</span>
                </span>
              </InfoTip>
            )}
            {series.map((s) => {
              const hidden = Boolean(hiddenStreams[s.id]);
              return (
                <button
                  key={s.id}
                  type="button"
                  onClick={() => toggleStream(s.id)}
                  aria-pressed={!hidden}
                  className={`inline-flex items-center gap-1.5 rounded border px-2 py-0.5 transition-opacity ${
                    hidden
                      ? "border-[var(--border-hairline)] bg-transparent text-[var(--text-muted)] opacity-45"
                      : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-primary)]"
                  }`}
                >
                  <span
                    className="inline-block h-2 w-2 rounded-full"
                    style={{ backgroundColor: s.color }}
                  />
                  <span className="max-w-[140px] truncate">{s.label}</span>
                </button>
              );
            })}
          </div>
        )}
      </div>

      <svg
        viewBox={`0 0 ${w} ${h}`}
        className="w-full h-auto select-none"
        role="img"
        aria-label="Concurrent viewers per stream on 60-second local timeline"
      >
        {yTicks.map((tick) => {
          const y = yFor(tick);
          return (
            <g key={tick}>
              <line
                x1={pad.left}
                x2={w - pad.right}
                y1={y}
                y2={y}
                stroke="var(--chart-grid)"
                strokeWidth="1"
              />
              <text
                x={pad.left - 8}
                y={y + 4}
                textAnchor="end"
                fill="var(--text-muted)"
                fontSize="11"
                fontFamily="IBM Plex Mono, monospace"
              >
                {tick >= 1000 ? `${(tick / 1000).toFixed(1)}k` : tick}
              </text>
            </g>
          );
        })}

        {matches.map((m) => {
          const startIdx = timeline.findIndex(
            (p) => new Date(p.bucket_utc).getTime() >= new Date(m.started_at).getTime()
          );
          if (startIdx < 0) return null;
          const endTime = m.ended_at ? new Date(m.ended_at).getTime() : Date.now();
          let endIdx = startIdx;
          for (let idx = timeline.length - 1; idx >= startIdx; idx--) {
            if (new Date(timeline[idx].bucket_utc).getTime() <= endTime) {
              endIdx = idx;
              break;
            }
          }
          const x1 = xFor(startIdx);
          const x2 = Math.max(x1 + 4, xFor(endIdx));
          return (
            <g key={m.id}>
              <rect
                x={x1}
                y={pad.top}
                width={x2 - x1}
                height={plotH}
                fill="var(--accent-soft)"
              />
              <text
                x={x1 + 4}
                y={pad.top + 12}
                fill="var(--accent-text)"
                fontSize="10"
                fontFamily="IBM Plex Mono, monospace"
              >
                {m.label}
              </text>
            </g>
          );
        })}

        {showCombined && (
          <>
            <path d={combinedAreaPath} fill="var(--accent-soft)" />
            <path
              d={combinedPath}
              fill="none"
              stroke="var(--accent)"
              strokeWidth={mode === "combined" ? "2.2" : "1.6"}
              strokeDasharray={mode === "streams_total" ? "4 3" : undefined}
              strokeLinejoin="round"
            />
          </>
        )}

        {showStreams &&
          visibleSeries.map((s) => {
            const d = timeline
              .map(
                (p, i) =>
                  `${i === 0 ? "M" : "L"}${xFor(i).toFixed(1)},${yFor(
                    p.per_channel_ccv?.[s.id] ?? 0
                  ).toFixed(1)}`
              )
              .join(" ");
            return (
              <path
                key={s.id}
                d={d}
                fill="none"
                stroke={s.color}
                strokeWidth="2"
                strokeLinejoin="round"
              />
            );
          })}

        {timeline.map((p, i) => (
          <g key={p.bucket_utc}>
            <rect
              x={xFor(i) - plotW / Math.max(timeline.length * 2, 2)}
              y={pad.top}
              width={plotW / Math.max(timeline.length, 1)}
              height={plotH}
              fill="transparent"
              onMouseEnter={() => setHoverIdx(i)}
              onMouseLeave={() => setHoverIdx(null)}
            />
            {hoverIdx === i && (
              <>
                <line
                  x1={xFor(i)}
                  x2={xFor(i)}
                  y1={pad.top}
                  y2={pad.top + plotH}
                  stroke="var(--chart-crosshair)"
                  strokeWidth="1"
                />
                {showCombined && (
                  <circle
                    cx={xFor(i)}
                    cy={yFor(p.combined_ccv)}
                    r="3.5"
                    fill="var(--bg-card)"
                    stroke="var(--accent)"
                    strokeWidth="2"
                  />
                )}
                {showStreams &&
                  visibleSeries.map((s) => (
                    <circle
                      key={s.id}
                      cx={xFor(i)}
                      cy={yFor(p.per_channel_ccv?.[s.id] ?? 0)}
                      r="3"
                      fill={s.color}
                    />
                  ))}
              </>
            )}
          </g>
        ))}

        <text
          x={pad.left}
          y={h - 8}
          fill="var(--text-muted)"
          fontSize="11"
          fontFamily="IBM Plex Mono, monospace"
        >
          {formatLocalHm(timeline[0].bucket_utc)}
        </text>
        <text
          x={w - pad.right}
          y={h - 8}
          textAnchor="end"
          fill="var(--text-muted)"
          fontSize="11"
          fontFamily="IBM Plex Mono, monospace"
        >
          {formatLocalHm(timeline[timeline.length - 1].bucket_utc)}
        </text>
      </svg>

      {hoverPoint && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 py-2 text-xs font-mono-tabular">
          <span className="text-[var(--text-secondary)]">
            {formatLocalHm(hoverPoint.bucket_utc)} · Combined{" "}
            <strong className="text-[var(--text-primary)]">
              {hoverPoint.combined_ccv.toLocaleString()}
            </strong>
          </span>
          <div className="flex flex-wrap items-center gap-3">
            {visibleSeries.map((s) => (
              <span key={s.id} className="inline-flex items-center gap-1.5 text-[var(--text-primary)]">
                <span
                  className="inline-block h-2 w-2 rounded-full"
                  style={{ backgroundColor: s.color }}
                />
                <span className="text-[var(--text-secondary)]">{s.label}:</span>
                <span>{(hoverPoint.per_channel_ccv?.[s.id] ?? 0).toLocaleString()}</span>
              </span>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

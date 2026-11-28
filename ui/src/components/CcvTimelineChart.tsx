import { useEffect, useId, useMemo, useRef, useState } from "react";
import { Sparkles, X } from "lucide-react";
import {
  ChannelRuntimeSnapshot,
  DashboardSnapshot,
  GridPoint,
  LanguageGroupConfig,
} from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { formatLocalHm } from "../utils/time";
import { ChartInspector } from "./ccv-chart/ChartInspector";
import {
  buildChartSeries,
  ChartMode,
  ChartSeries,
  computeMatchBands,
  computeXTickIndices,
  formatCompactAxis,
  niceAxisMax,
} from "./ccv-chart/chartUtils";
import { InfoTip } from "./InfoTip";

const CHART_W = 820;
const CHART_H = 300;
const PAD = { top: 28, right: 20, bottom: 34, left: 54 };
const PLOT_W = CHART_W - PAD.left - PAD.right;
const PLOT_H = CHART_H - PAD.top - PAD.bottom;

export function CcvTimelineChart({
  timeline,
  matches,
  channels = [],
  groups = [],
}: {
  timeline: GridPoint[];
  matches: DashboardSnapshot["matches"];
  channels?: ChannelRuntimeSnapshot[];
  groups?: LanguageGroupConfig[];
}) {
  const gradId = useId();
  const svgRef = useRef<SVGSVGElement | null>(null);
  const [mode, setMode] = useState<ChartMode>(() =>
    groups.length > 0 ? "category" : "top_streams"
  );
  const [hoverIdx, setHoverIdx] = useState<number | null>(null);
  const [pinnedIdx, setPinnedIdx] = useState<number | null>(null);
  const [isDragging, setIsDragging] = useState(false);
  const [hiddenIds, setHiddenIds] = useState<Record<string, boolean>>({});
  const [legendExpanded, setLegendExpanded] = useState(false);

  useEffect(() => {
    if (pinnedIdx !== null && pinnedIdx >= timeline.length) {
      setPinnedIdx(null);
    }
    if (hoverIdx !== null && hoverIdx >= timeline.length) {
      setHoverIdx(null);
    }
  }, [hoverIdx, pinnedIdx, timeline.length]);

  const { channelSeries, categorySeries, peakIdx } = useMemo(
    () => buildChartSeries(timeline, channels, groups),
    [channels, groups, timeline]
  );

  const xTickIndices = useMemo(() => computeXTickIndices(timeline.length), [timeline.length]);

  const xFor = (i: number) =>
    PAD.left + (timeline.length <= 1 ? PLOT_W / 2 : (i / (timeline.length - 1)) * PLOT_W);

  const matchBands = useMemo(
    () => computeMatchBands(timeline, matches, xFor),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [matches, timeline]
  );

  if (timeline.length === 0) {
    return (
      <div className="flex h-[300px] items-center justify-center rounded-lg border border-dashed border-[var(--border-hairline)] text-sm text-[var(--text-muted)]">
        Waiting for first 60-second accounting interval
      </div>
    );
  }

  const activeSeriesList: ChartSeries[] =
    mode === "category"
      ? categorySeries
      : mode === "top_streams"
      ? channelSeries.slice(0, 8)
      : mode === "all_streams"
      ? channelSeries
      : [];
  const visibleSeries = activeSeriesList.filter((s) => !hiddenIds[s.id]);

  let rawMax = 100;
  for (const pt of timeline) {
    if (pt.combined_ccv > rawMax) rawMax = pt.combined_ccv;
    for (const s of visibleSeries) {
      const v = s.getValue(pt);
      if (v > rawMax) rawMax = v;
    }
  }
  const maxVal = niceAxisMax(rawMax);
  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((t) => Math.round(maxVal * t));
  const yFor = (val: number) => PAD.top + PLOT_H - (val / maxVal) * PLOT_H;

  const combinedPath = timeline
    .map((p, i) => `${i === 0 ? "M" : "L"}${xFor(i).toFixed(1)},${yFor(p.combined_ccv).toFixed(1)}`)
    .join(" ");
  const combinedAreaPath = `${combinedPath} L${xFor(timeline.length - 1).toFixed(1)},${(
    PAD.top + PLOT_H
  ).toFixed(1)} L${xFor(0).toFixed(1)},${(PAD.top + PLOT_H).toFixed(1)} Z`;

  const idxFromClientX = (clientX: number): number => {
    const svg = svgRef.current;
    if (!svg || timeline.length <= 1) return 0;
    const rect = svg.getBoundingClientRect();
    if (rect.width <= 0) return 0;
    const svgX = ((clientX - rect.left) / rect.width) * CHART_W;
    const ratio = Math.max(0, Math.min(1, (svgX - PAD.left) / PLOT_W));
    return Math.round(ratio * (timeline.length - 1));
  };

  const rawActiveIdx =
    hoverIdx !== null ? hoverIdx : pinnedIdx !== null ? pinnedIdx : timeline.length - 1;
  const activeIdx = Math.max(0, Math.min(timeline.length - 1, rawActiveIdx));
  const activePoint = timeline[activeIdx];
  const isShowingPinned =
    pinnedIdx !== null && !isDragging && (hoverIdx === null || hoverIdx === pinnedIdx);
  const isShowingLatest = hoverIdx === null && pinnedIdx === null;
  const activeMatch = matchBands.find((b) => activeIdx >= b.startIdx && activeIdx <= b.endIdx);

  const legendItems =
    mode === "all_streams" && !legendExpanded
      ? activeSeriesList.slice(0, 12)
      : activeSeriesList;

  const modeTabs: readonly (readonly [ChartMode, string])[] = [
    ...(groups.length > 0 ? ([["category", "By Category"]] as const) : []),
    ["top_streams", "Top 8 Streams"],
    ["all_streams", "All Streams"],
    ["combined", "Total Only"],
  ];

  return (
    <div className="space-y-3">
      {/* Mode Switcher + Peak / Unpin Controls */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-[11px]">
        <div
          role="tablist"
          aria-label="Chart display mode"
          className="inline-flex rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] p-0.5 font-mono-tabular"
        >
          {modeTabs.map(([val, label]) => (
            <button
              key={val}
              type="button"
              role="tab"
              aria-selected={mode === val}
              onClick={() => setMode(val)}
              className={`rounded-md px-2.5 py-1 transition-colors ${
                mode === val
                  ? "bg-[var(--bg-card)] font-medium text-[var(--text-primary)] shadow-sm"
                  : "text-[var(--text-muted)] hover:text-[var(--text-secondary)]"
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        <div className="flex flex-wrap items-center gap-1.5 font-mono-tabular">
          <button
            type="button"
            onClick={() => {
              setHoverIdx(null);
              setPinnedIdx(peakIdx);
            }}
            className="inline-flex items-center gap-1 rounded-md border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-1 text-[var(--text-secondary)] transition-colors hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
            title="Pin inspector to Peak CCV timestamp"
          >
            <Sparkles size={11} strokeWidth={1.75} className="text-[var(--accent)]" aria-hidden="true" />
            <span>
              Peak {timeline[peakIdx].combined_ccv.toLocaleString()} (
              {formatLocalHm(timeline[peakIdx].bucket_utc)})
            </span>
          </button>
          {pinnedIdx !== null && (
            <button
              type="button"
              onClick={() => setPinnedIdx(null)}
              className="inline-flex items-center gap-1 rounded-md border border-[var(--accent)]/45 bg-[var(--accent-soft)] px-2 py-1 text-[var(--accent-text)] transition-colors hover:opacity-90"
            >
              <X size={11} strokeWidth={2} aria-hidden="true" />
              <span>Unpin</span>
            </button>
          )}
        </div>
      </div>

      {/* Interactive Legend */}
      {activeSeriesList.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
          <InfoTip entry={ANALYTICS_GLOSSARY.ccv} focusable>
            <span className="inline-flex items-center gap-1.5 rounded border border-[var(--accent)]/35 bg-[var(--accent-soft)]/50 px-2 py-0.5 font-medium text-[var(--text-primary)]">
              <span className="inline-block h-0.5 w-3 bg-[var(--accent)]" />
              <span>Combined Total</span>
            </span>
          </InfoTip>
          {legendItems.map((s) => {
            const hidden = Boolean(hiddenIds[s.id]);
            return (
              <button
                key={s.id}
                type="button"
                onClick={() => setHiddenIds((prev) => ({ ...prev, [s.id]: !prev[s.id] }))}
                aria-pressed={!hidden}
                className={`inline-flex items-center gap-1.5 rounded border px-2 py-0.5 transition-opacity ${
                  hidden
                    ? "border-[var(--border-hairline)] bg-transparent text-[var(--text-muted)] opacity-45"
                    : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-primary)] hover:border-[var(--border-strong)]"
                }`}
              >
                <span
                  className="inline-block h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: s.color }}
                />
                <span className="max-w-[160px] truncate">{s.label}</span>
              </button>
            );
          })}
          {mode === "all_streams" && activeSeriesList.length > 12 && (
            <button
              type="button"
              onClick={() => setLegendExpanded((prev) => !prev)}
              className="rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2 py-0.5 text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              {legendExpanded ? "Show fewer" : `+${activeSeriesList.length - 12} more`}
            </button>
          )}
        </div>
      )}

      {/* Main SVG Plot */}
      <div
        tabIndex={0}
        role="application"
        aria-label="Live viewers timeline chart. Click or drag to pin a timestamp; use Left and Right arrow keys to scrub."
        onKeyDown={(e) => {
          if (e.key === "ArrowLeft") {
            e.preventDefault();
            setHoverIdx(null);
            setPinnedIdx((prev) => Math.max(0, (prev ?? activeIdx) - 1));
          } else if (e.key === "ArrowRight") {
            e.preventDefault();
            setHoverIdx(null);
            setPinnedIdx((prev) => Math.min(timeline.length - 1, (prev ?? activeIdx) + 1));
          } else if (e.key === "Escape") {
            setPinnedIdx(null);
            setHoverIdx(null);
          }
        }}
        className="rounded-lg border border-[var(--border-hairline)]/60 bg-[var(--bg-page)]/40 p-1 focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--accent)]"
      >
        <svg
          ref={svgRef}
          viewBox={`0 0 ${CHART_W} ${CHART_H}`}
          className="h-auto w-full cursor-crosshair select-none touch-none"
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            const idx = idxFromClientX(e.clientX);
            setIsDragging(true);
            setHoverIdx(idx);
            setPinnedIdx(idx);
          }}
          onPointerMove={(e) => {
            const idx = idxFromClientX(e.clientX);
            setHoverIdx(idx);
            if (isDragging) setPinnedIdx(idx);
          }}
          onPointerUp={(e) => {
            if (e.currentTarget.hasPointerCapture(e.pointerId)) {
              e.currentTarget.releasePointerCapture(e.pointerId);
            }
            setPinnedIdx(idxFromClientX(e.clientX));
            setIsDragging(false);
          }}
          onPointerCancel={() => setIsDragging(false)}
          onPointerLeave={() => {
            if (!isDragging) setHoverIdx(null);
          }}
        >
          <defs>
            <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--accent)" stopOpacity="0.24" />
              <stop offset="100%" stopColor="var(--accent)" stopOpacity="0.01" />
            </linearGradient>
            {matchBands.map((b) => (
              <clipPath key={`clip-${b.id}`} id={`clip-${gradId}-${b.id}`}>
                <rect x={b.x1 + 2} y={PAD.top - 18} width={Math.max(0, b.width - 4)} height={18} />
              </clipPath>
            ))}
          </defs>

          {yTicks.map((tick) => {
            const y = yFor(tick);
            return (
              <g key={tick}>
                <line
                  x1={PAD.left}
                  x2={CHART_W - PAD.right}
                  y1={y}
                  y2={y}
                  stroke="var(--chart-grid)"
                  strokeWidth="1"
                  strokeDasharray={tick === 0 ? undefined : "2 3"}
                />
                <text
                  x={PAD.left - 8}
                  y={y + 4}
                  textAnchor="end"
                  fill="var(--text-muted)"
                  fontSize="10.5"
                  fontFamily="IBM Plex Mono, monospace"
                >
                  {formatCompactAxis(tick)}
                </text>
              </g>
            );
          })}

          {matchBands.map((b) => {
            const isHighlighted = activeMatch?.id === b.id;
            return (
              <g key={b.id}>
                <rect
                  x={b.x1}
                  y={PAD.top}
                  width={b.width}
                  height={PLOT_H}
                  fill="var(--accent)"
                  fillOpacity={isHighlighted ? 0.14 : 0.06}
                />
                <rect
                  x={b.x1}
                  y={PAD.top - 16}
                  width={b.width}
                  height={16}
                  rx={2}
                  fill="var(--accent)"
                  fillOpacity={isHighlighted ? 0.28 : 0.12}
                />
                {b.width >= 44 && (
                  <text
                    x={b.x1 + 5}
                    y={PAD.top - 4}
                    fill="var(--accent-text)"
                    fontSize="9.5"
                    fontFamily="IBM Plex Mono, monospace"
                    clipPath={`url(#clip-${gradId}-${b.id})`}
                  >
                    {b.label}
                  </text>
                )}
              </g>
            );
          })}

          <path d={combinedAreaPath} fill={`url(#${gradId})`} />
          <path
            d={combinedPath}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={mode === "combined" ? "2.4" : "1.8"}
            strokeDasharray={mode === "combined" ? undefined : "5 3"}
            strokeLinejoin="round"
          />

          {visibleSeries.map((s) => {
            const d = timeline
              .map(
                (p, i) =>
                  `${i === 0 ? "M" : "L"}${xFor(i).toFixed(1)},${yFor(s.getValue(p)).toFixed(1)}`
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
                strokeOpacity="0.92"
              />
            );
          })}

          <circle
            cx={xFor(peakIdx)}
            cy={yFor(timeline[peakIdx].combined_ccv)}
            r="3"
            fill="var(--accent)"
            fillOpacity="0.85"
          />

          <g className="pointer-events-none">
            <line
              x1={xFor(activeIdx)}
              x2={xFor(activeIdx)}
              y1={PAD.top}
              y2={PAD.top + PLOT_H}
              stroke={isShowingPinned ? "var(--accent)" : "var(--chart-crosshair)"}
              strokeWidth={isShowingPinned ? "1.5" : "1"}
              strokeDasharray={isShowingPinned ? undefined : "3 3"}
            />
            <circle
              cx={xFor(activeIdx)}
              cy={yFor(activePoint.combined_ccv)}
              r="4.5"
              fill="var(--bg-card)"
              stroke="var(--accent)"
              strokeWidth="2.2"
            />
            {visibleSeries.map((s) => (
              <circle
                key={s.id}
                cx={xFor(activeIdx)}
                cy={yFor(s.getValue(activePoint))}
                r="3.2"
                fill={s.color}
                stroke="var(--bg-card)"
                strokeWidth="1"
              />
            ))}
          </g>

          {xTickIndices.map((idx, pos) => {
            const anchor =
              pos === 0 ? "start" : pos === xTickIndices.length - 1 ? "end" : "middle";
            return (
              <text
                key={idx}
                x={xFor(idx)}
                y={CHART_H - 9}
                textAnchor={anchor}
                fill="var(--text-muted)"
                fontSize="10.5"
                fontFamily="IBM Plex Mono, monospace"
              >
                {formatLocalHm(timeline[idx].bucket_utc)}
              </text>
            );
          })}
        </svg>
      </div>

      <ChartInspector
        activePoint={activePoint}
        activeMatch={activeMatch}
        series={mode === "combined" ? categorySeries : activeSeriesList}
        hiddenIds={hiddenIds}
        isDragging={isDragging}
        isShowingPinned={isShowingPinned}
        isShowingLatest={isShowingLatest}
      />
    </div>
  );
}

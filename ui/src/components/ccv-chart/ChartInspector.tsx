import { Pin } from "lucide-react";
import { GridPoint } from "../../types";
import { formatLocalHm } from "../../utils/time";
import { ChartSeries, MatchBand } from "./chartUtils";

interface ChartInspectorProps {
  activePoint: GridPoint;
  activeMatch?: MatchBand;
  series: ChartSeries[];
  hiddenIds: Record<string, boolean>;
  isShowingPinned: boolean;
  isShowingLatest: boolean;
}

export function ChartInspector({
  activePoint,
  activeMatch,
  series,
  hiddenIds,
  isShowingPinned,
  isShowingLatest,
}: ChartInspectorProps) {
  const rows = series
    .map((s) => ({
      id: s.id,
      label: s.label,
      color: s.color,
      value: s.getValue(activePoint),
      hidden: Boolean(hiddenIds[s.id]),
    }))
    .sort((a, b) => b.value - a.value);

  const nonZeroRows = rows.filter((r) => r.value > 0);
  const zeroRowCount = rows.length - nonZeroRows.length;

  return (
    <div className="rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3.5 py-2.5 text-xs">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-[var(--border-hairline)]/70 pb-2 font-mono-tabular">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-semibold text-[var(--text-primary)]">
            {formatLocalHm(activePoint.bucket_utc)}
          </span>
          <span className="text-[var(--text-muted)]">·</span>
          <span className="text-[var(--text-secondary)]">
            Combined{" "}
            <strong className="text-sm text-[var(--text-primary)]">
              {activePoint.combined_ccv.toLocaleString()}
            </strong>
          </span>
          {activeMatch && (
            <span className="rounded border border-[var(--accent)]/40 bg-[var(--accent-soft)] px-2 py-0.5 text-[10.5px] font-medium text-[var(--accent-text)]">
              {activeMatch.label}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1.5 text-[10.5px] text-[var(--text-muted)]">
          {isShowingPinned ? (
            <span className="inline-flex items-center gap-1 rounded border border-[var(--accent)]/40 bg-[var(--accent-soft)]/70 px-1.5 py-0.5 text-[var(--accent-text)]">
              <Pin size={10} strokeWidth={2} aria-hidden="true" />
              <span>Pinned · drag or click chart to move</span>
            </span>
          ) : isShowingLatest ? (
            <span>Latest interval · click or drag chart to pin</span>
          ) : (
            <span>Scrubbing · release to pin</span>
          )}
        </div>
      </div>

      {nonZeroRows.length > 0 ? (
        <div className="mt-2 grid grid-cols-1 gap-x-4 gap-y-1.5 font-mono-tabular sm:grid-cols-2 xl:grid-cols-3">
          {nonZeroRows.map((row) => {
            const pct =
              activePoint.combined_ccv > 0
                ? (row.value / activePoint.combined_ccv) * 100
                : 0;
            return (
              <div
                key={row.id}
                className={`flex items-center justify-between gap-2 text-[11px] ${
                  row.hidden ? "opacity-45" : ""
                }`}
              >
                <span className="flex min-w-0 items-center gap-1.5">
                  <span
                    className="inline-block h-2 w-2 shrink-0 rounded-full"
                    style={{ backgroundColor: row.color }}
                  />
                  <span className="truncate text-[var(--text-secondary)]">{row.label}</span>
                </span>
                <span className="shrink-0 text-[var(--text-primary)]">
                  {row.value.toLocaleString()}{" "}
                  <span className="text-[10px] text-[var(--text-muted)]">
                    ({pct.toFixed(0)}%)
                  </span>
                </span>
              </div>
            );
          })}
        </div>
      ) : (
        <div className="mt-2 text-[11px] text-[var(--text-muted)]">
          No active viewer counts recorded at this interval.
        </div>
      )}
      {zeroRowCount > 0 && nonZeroRows.length > 0 && (
        <div className="mt-1.5 text-[10.5px] text-[var(--text-muted)]">
          + {zeroRowCount} {zeroRowCount === 1 ? "stream" : "streams"} at 0 viewers
        </div>
      )}
    </div>
  );
}

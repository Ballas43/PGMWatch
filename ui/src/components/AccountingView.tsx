import { useEffect, useState } from "react";
import { CalendarClock, RotateCcw, X } from "lucide-react";
import { AccountingResponse, DashboardSnapshot } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { getDeviceTimeZoneLabel } from "../utils/time";
import { formatTriStateCompact } from "../utils/telemetry";
import { CcvTimelineChart } from "./CcvTimelineChart";
import { InfoTip } from "./InfoTip";
import { ManualOverrideForm } from "./ManualOverrideForm";

function isoToLocalInput(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function parseLocalDatetimeInput(val: string): { ok: true; iso: string | null } | { ok: false; error: string } {
  const trimmed = val.trim();
  if (!trimmed) return { ok: true, iso: null };
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(trimmed);
  if (!m) return { ok: false, error: "Invalid date/time format." };
  const d = new Date(
    Number(m[1]),
    Number(m[2]) - 1,
    Number(m[3]),
    Number(m[4]),
    Number(m[5]),
    m[6] ? Number(m[6]) : 0
  );
  if (Number.isNaN(d.getTime())) return { ok: false, error: "Invalid date/time value." };
  return { ok: true, iso: d.toISOString() };
}

interface AccountingViewProps {
  snapshot: DashboardSnapshot | null;
  onRefresh: () => void;
}

export function AccountingView({ snapshot, onRefresh }: AccountingViewProps) {
  const [accounting, setAccounting] = useState<AccountingResponse | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [startInput, setStartInput] = useState(() => isoToLocalInput(snapshot?.show.scheduled_start_at));
  const [endInput, setEndInput] = useState(() => isoToLocalInput(snapshot?.show.scheduled_end_at));
  const [dirtyWindow, setDirtyWindow] = useState(false);
  const [savingWindow, setSavingWindow] = useState(false);
  const [windowErr, setWindowErr] = useState<string | null>(null);

  const showId = snapshot?.show.show_id;
  const savedStart = snapshot?.show.scheduled_start_at ?? null;
  const savedEnd = snapshot?.show.scheduled_end_at ?? null;

  useEffect(() => {
    setDirtyWindow(false);
    setStartInput(isoToLocalInput(savedStart));
    setEndInput(isoToLocalInput(savedEnd));
    setWindowErr(null);
  }, [showId]);

  useEffect(() => {
    if (!dirtyWindow) {
      setStartInput(isoToLocalInput(savedStart));
      setEndInput(isoToLocalInput(savedEnd));
    }
  }, [savedStart, savedEnd, dirtyWindow]);

  const loadAccounting = async () => {
    const res = await fetch("/api/accounting");
    if (res.ok) {
      setAccounting(await res.json());
    }
  };

  useEffect(() => {
    void loadAccounting();
    const t = window.setInterval(() => void loadAccounting(), 10000);
    return () => window.clearInterval(t);
  }, []);

  const saveScheduleWindow = async (nextStart: string | null, nextEnd: string | null) => {
    setSavingWindow(true);
    setWindowErr(null);
    try {
      const res = await fetch("/api/show/schedule", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          scheduled_start_at: nextStart,
          scheduled_end_at: nextEnd,
        }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setWindowErr(body.error ?? `Failed to save window (HTTP ${res.status})`);
        return;
      }
      setDirtyWindow(false);
      await loadAccounting();
      onRefresh();
    } catch (err) {
      setWindowErr(err instanceof Error ? err.message : "Network error");
    } finally {
      setSavingWindow(false);
    }
  };

  const resetGraph = async () => {
    if (!confirmReset) {
      setConfirmReset(true);
      window.setTimeout(() => setConfirmReset(false), 4000);
      return;
    }
    setResetting(true);
    try {
      const res = await fetch("/api/accounting/reset", { method: "POST" });
      if (res.ok) {
        setConfirmReset(false);
        await loadAccounting();
        onRefresh();
      }
    } finally {
      setResetting(false);
    }
  };

  const sum = accounting?.summary;

  return (
    <div className="space-y-4 animate-enter">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <h1 className="text-lg font-medium tracking-tight text-[var(--text-primary)]">
            Broadcast Accounting
          </h1>
          <p className="text-xs text-[var(--text-secondary)]">
            1-minute viewer summaries ({getDeviceTimeZoneLabel()}), show segments, and manual viewer corrections
          </p>
        </div>

        <button
          type="button"
          disabled={resetting}
          onClick={() => void resetGraph()}
          className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs font-medium transition-colors ${
            confirmReset
              ? "border-[var(--negative)] bg-[var(--negative-soft)] text-[var(--negative-text)]"
              : "border-[var(--border-hairline)] bg-[var(--bg-card)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
          }`}
        >
          <RotateCcw size={13} strokeWidth={1.5} />
          <span>{resetting ? "Resetting..." : confirmReset ? "Click Again to Confirm Reset" : "Reset Graph"}</span>
        </button>
      </div>

      <section className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] px-4 py-3">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <CalendarClock size={14} strokeWidth={1.5} className="text-[var(--text-muted)]" aria-hidden="true" />
            <h2 className="text-xs font-semibold text-[var(--text-primary)]">
              Broadcast Push & End Window ({getDeviceTimeZoneLabel()})
            </h2>
            {(savedStart || savedEnd) ? (
              <span className="rounded border border-[var(--positive)]/40 bg-[var(--positive-soft)] px-2 py-0.5 text-[11px] font-mono-tabular text-[var(--positive-text)]">
                Window active · pre-show & post-show excluded from analytics & Excel
              </span>
            ) : (
              <span className="text-xs text-[var(--text-muted)]">
                Optional — set when the engineer pushes & ends the stream for exact PCV/ACV/Hours Watched
              </span>
            )}
          </div>
          {(savedStart || savedEnd || startInput || endInput) && (
            <button
              type="button"
              disabled={savingWindow}
              onClick={() => {
                setStartInput("");
                setEndInput("");
                void saveScheduleWindow(null, null);
              }}
              className="inline-flex h-7 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-secondary)] hover:text-[var(--text-primary)] disabled:opacity-50"
            >
              <X size={12} strokeWidth={1.5} />
              <span>Clear Window</span>
            </button>
          )}
        </div>

        <form
          onSubmit={(e) => {
            e.preventDefault();
            const parsedStart = parseLocalDatetimeInput(startInput);
            if (!parsedStart.ok) {
              setWindowErr(`Start time: ${parsedStart.error}`);
              return;
            }
            const parsedEnd = parseLocalDatetimeInput(endInput);
            if (!parsedEnd.ok) {
              setWindowErr(`End time: ${parsedEnd.error}`);
              return;
            }
            void saveScheduleWindow(parsedStart.iso, parsedEnd.iso);
          }}
          className="mt-2.5 flex flex-wrap items-end gap-2.5"
        >
          <div className="flex flex-col gap-1 text-[11px] text-[var(--text-muted)]">
            <label htmlFor="sched-start-input">Stream Push (Start)</label>
            <div className="flex items-center gap-1">
              <input
                id="sched-start-input"
                type="datetime-local"
                value={startInput}
                onChange={(e) => {
                  setStartInput(e.target.value);
                  setDirtyWindow(true);
                }}
                className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs font-mono-tabular text-[var(--text-primary)]"
              />
              <button
                type="button"
                onClick={() => {
                  setStartInput(isoToLocalInput(new Date().toISOString()));
                  setDirtyWindow(true);
                }}
                className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                title="Set start time to current time"
              >
                Now
              </button>
            </div>
          </div>

          <div className="flex flex-col gap-1 text-[11px] text-[var(--text-muted)]">
            <label htmlFor="sched-end-input">Stream End</label>
            <div className="flex items-center gap-1">
              <input
                id="sched-end-input"
                type="datetime-local"
                value={endInput}
                onChange={(e) => {
                  setEndInput(e.target.value);
                  setDirtyWindow(true);
                }}
                className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs font-mono-tabular text-[var(--text-primary)]"
              />
              <button
                type="button"
                onClick={() => {
                  setEndInput(isoToLocalInput(new Date().toISOString()));
                  setDirtyWindow(true);
                }}
                className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
                title="Set end time to current time"
              >
                Now
              </button>
            </div>
          </div>

          <button
            type="submit"
            disabled={savingWindow}
            className="inline-flex h-8 items-center rounded-lg bg-[var(--text-primary)] px-3 text-xs font-medium text-[var(--bg-rail)] transition-opacity hover:opacity-90 disabled:opacity-50"
          >
            {savingWindow ? "Saving..." : dirtyWindow ? "Apply Window *" : "Apply Window"}
          </button>

          {windowErr && (
            <span className="text-xs text-[var(--negative-text)]" role="alert">
              {windowErr}
            </span>
          )}
        </form>
      </section>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5">
          <div className="text-xs text-[var(--text-muted)]">
            <InfoTip entry={ANALYTICS_GLOSSARY.pcv} underline focusable>
              Peak Live Viewers
            </InfoTip>
          </div>
          <div className="mt-2 text-3xl font-medium font-mono-tabular text-[var(--text-primary)]">
            {(sum?.combined_pcv ?? snapshot?.combined_pcv ?? 0).toLocaleString()}
          </div>
          <div className="mt-1 text-xs text-[var(--text-secondary)] font-mono-tabular">
            Across {sum?.total_minutes ?? 0} tracked 1-min intervals
          </div>
        </div>

        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5">
          <div className="text-xs text-[var(--text-muted)]">
            <InfoTip entry={ANALYTICS_GLOSSARY.hw} underline focusable>
              Total Hours Watched
            </InfoTip>
          </div>
          <div className="mt-2 text-3xl font-medium font-mono-tabular text-[var(--text-primary)]">
            {(sum?.hours_watched ?? snapshot?.hours_watched ?? 0).toFixed(1)}
          </div>
          <div className="mt-1 text-xs text-[var(--text-secondary)] font-mono-tabular">
            Sum of 1-minute viewer buckets ÷ 60
          </div>
        </div>

        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5">
          <div className="text-xs text-[var(--text-muted)]">
            <InfoTip entry={ANALYTICS_GLOSSARY.acv} underline focusable>
              Average Live Viewers
            </InfoTip>
          </div>
          <div className="mt-2 text-3xl font-medium font-mono-tabular text-[var(--text-primary)]">
            {Math.round(sum?.combined_acv ?? 0).toLocaleString()}
          </div>
          <div className="mt-1 text-xs text-[var(--text-secondary)] font-mono-tabular">
            Unavailable metrics excluded from averages
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 lg:col-span-2">
          <div className="mb-3 flex items-center justify-between">
            <h2 className="text-sm font-medium text-[var(--text-primary)]">
              <InfoTip entry={ANALYTICS_GLOSSARY.gridBucket} underline focusable>
                Live Viewers Over Time (1-Min Grid)
              </InfoTip>
            </h2>
            <span className="text-xs font-mono-tabular text-[var(--text-muted)]">
              {accounting?.matches.length ?? 0} segment bands
            </span>
          </div>
          <CcvTimelineChart
            timeline={sum?.timeline ?? []}
            matches={snapshot?.matches ?? []}
            channels={snapshot?.channels ?? []}
          />
        </div>

        <div className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5">
          <h2 className="mb-3 text-sm font-medium text-[var(--text-primary)]">
            Group Totals
          </h2>
          <div className="space-y-2.5">
            {(snapshot?.show.groups ?? []).map((g) => {
              const pcv = sum?.group_pcv[g.id] ?? 0;
              const hw = sum?.group_hours_watched[g.id] ?? 0;
              return (
                <div
                  key={g.id}
                  className="flex items-center justify-between rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 py-2.5"
                >
                  <div>
                    <div className="text-sm font-medium text-[var(--text-primary)]">
                      {g.language_code}
                    </div>
                    <div className="text-xs text-[var(--text-muted)]">{g.name}</div>
                  </div>
                  <div className="text-right font-mono-tabular">
                    <div className="text-sm text-[var(--text-primary)]">
                      <InfoTip entry={ANALYTICS_GLOSSARY.pcv} underline focusable>
                        {pcv.toLocaleString()} Peak
                      </InfoTip>
                    </div>
                    <div className="text-xs text-[var(--text-muted)]">
                      <InfoTip entry={ANALYTICS_GLOSSARY.hw} underline focusable>
                        {hw.toFixed(1)} Watch Hrs
                      </InfoTip>
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-3">
        <div className="overflow-x-auto rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 xl:col-span-2">
          <h2 className="mb-3 text-sm font-medium text-[var(--text-primary)]">
            Channel Data Audit
          </h2>
          <table className="w-full text-left text-xs">
            <thead>
              <tr className="border-b border-[var(--border-hairline)] text-[var(--text-muted)]">
                <th scope="col" className="pb-2 pr-2 font-medium">Channel</th>
                <th scope="col" className="pb-2 pr-2 font-medium">
                  <InfoTip entry={ANALYTICS_GLOSSARY.metricStatus} underline focusable>Status</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.ccv} underline focusable>Live Viewers</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.views} underline focusable>Total Views</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.likes} underline focusable>Likes</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.chatRate} underline focusable>Chat / Min</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.shares} underline focusable>Shares</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.gifts} underline focusable>Gifts</InfoTip>
                </th>
                <th scope="col" className="pb-2 pr-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.coverage} underline focusable>Coverage</InfoTip>
                </th>
                <th scope="col" className="pb-2 font-medium text-right">
                  <InfoTip entry={ANALYTICS_GLOSSARY.metricSource} underline focusable>Source</InfoTip>
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-[var(--border-hairline)] font-mono-tabular">
              {(snapshot?.channels ?? []).map((ch) => (
                <tr key={ch.config.id} className="hover:bg-[var(--bg-raised)]/40">
                  <td className="py-2.5 pr-2 font-sans text-[var(--text-primary)]">
                    <span>{ch.config.name}</span>
                    {ch.config.exclude_from_accounting && (
                      <span className="ml-1.5 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-1 py-0.5 font-mono-tabular text-[10px] text-[var(--text-muted)]">
                        Excluded
                      </span>
                    )}
                    {(ch.config.broadcast_delay_secs ?? 0) > 0 && (
                      <span className="ml-1.5 font-mono-tabular text-[10px] text-[var(--accent-text)]">
                        +{ch.config.broadcast_delay_secs}s
                      </span>
                    )}
                  </td>
                  <td className="py-2.5 pr-2">
                    <span
                      title={ch.ccv.note ?? ch.ccv.status}
                      className={
                        ch.ccv.status === "VALUE"
                          ? "text-[var(--positive-text)]"
                          : ch.ccv.status === "UNAVAILABLE"
                          ? "text-[var(--warning-text)]"
                          : "text-[var(--text-muted)]"
                      }
                    >
                      {ch.ccv.status === "VALUE"
                        ? "Live"
                        : ch.ccv.status === "UNAVAILABLE"
                        ? "Unavailable"
                        : "N/A"}
                    </span>
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-primary)]">
                    {ch.ccv.value !== null
                      ? ch.ccv.value.toLocaleString()
                      : ch.ccv.status === "UNAVAILABLE"
                      ? "Unavailable"
                      : "N/A"}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]" title={ch.total_views.note ?? undefined}>
                    {formatTriStateCompact(ch.total_views)}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]" title={ch.likes.note ?? undefined}>
                    {formatTriStateCompact(ch.likes)}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]" title={ch.chat_rate_per_min.note ?? undefined}>
                    {formatTriStateCompact(ch.chat_rate_per_min)}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]" title={ch.shares.note ?? undefined}>
                    {formatTriStateCompact(ch.shares)}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]" title={ch.gifts_count.note ?? undefined}>
                    {formatTriStateCompact(ch.gifts_count)}
                  </td>
                  <td className="py-2.5 pr-2 text-right text-[var(--text-secondary)]">
                    {ch.ccv.coverage_pct.toFixed(0)}%
                  </td>
                  <td className="py-2.5 text-right text-[var(--text-muted)]">
                    {ch.ccv.source === "MANUAL_STUDIO" ? "Studio Manual" : "Auto"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <ManualOverrideForm
          channels={snapshot?.channels ?? []}
          onApplied={async () => {
            await loadAccounting();
            onRefresh();
          }}
        />
      </div>
    </div>
  );
}

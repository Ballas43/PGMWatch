import { useEffect, useState } from "react";
import { RotateCcw } from "lucide-react";
import { AccountingResponse, DashboardSnapshot } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { getDeviceTimeZoneLabel } from "../utils/time";
import { formatTriStateCompact } from "../utils/telemetry";
import { CcvTimelineChart } from "./CcvTimelineChart";
import { InfoTip } from "./InfoTip";
import { ManualOverrideForm } from "./ManualOverrideForm";

interface AccountingViewProps {
  snapshot: DashboardSnapshot | null;
  onRefresh: () => void;
}

export function AccountingView({ snapshot, onRefresh }: AccountingViewProps) {
  const [accounting, setAccounting] = useState<AccountingResponse | null>(null);
  const [confirmReset, setConfirmReset] = useState(false);
  const [resetting, setResetting] = useState(false);

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
    <div className="space-y-6 animate-enter">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-2xl font-medium tracking-tight text-[var(--text-primary)]">
            Broadcast Accounting
          </h1>
            <p className="mt-1 text-sm text-[var(--text-secondary)]">
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

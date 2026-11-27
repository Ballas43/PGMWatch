import { useState } from "react";
import { Activity, BatteryWarning, ChevronDown, ChevronUp, Cpu, ShieldAlert } from "lucide-react";
import type { GovernorTelemetry } from "../types";
import { ANALYTICS_GLOSSARY, type MetricGlossaryEntry } from "../utils/analyticsGlossary";
import { InfoTip } from "./InfoTip";

interface GovernorStatusStripProps {
  governor: GovernorTelemetry;
}

function barTone(pct: number): string {
  if (pct >= 88) return "bg-[var(--negative)]";
  if (pct >= 75) return "bg-[var(--warning)]";
  return "bg-[var(--positive)]";
}

function MeterCell({
  label,
  pct,
  detail,
  tip,
}: {
  label: string;
  pct: number;
  detail: string;
  tip: MetricGlossaryEntry;
}) {
  const clamped = Math.max(0, Math.min(100, Number.isFinite(pct) ? pct : 0));
  return (
    <InfoTip entry={tip} focusable className="block cursor-help">
      <div className="rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)]/60 px-2.5 py-1.5">
        <div className="flex items-center justify-between gap-2 text-xs">
          <span className="text-[var(--text-muted)] underline decoration-[var(--border-strong)] decoration-dotted underline-offset-[3px]">
            {label}
          </span>
          <span className="font-mono-tabular font-medium text-[var(--text-primary)]">
            {clamped.toFixed(0)}%
          </span>
        </div>
        <div
          className="mt-1 h-1.5 w-full overflow-hidden rounded-full bg-[var(--bg-rail)]"
          role="progressbar"
          aria-label={label}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(clamped)}
        >
          <div
            className={`h-full transition-[width] duration-200 ${barTone(clamped)}`}
            style={{ width: `${clamped}%` }}
          />
        </div>
        <div className="mt-0.5 truncate text-[11px] font-mono-tabular text-[var(--text-muted)]">
          {detail}
        </div>
      </div>
    </InfoTip>
  );
}

export function GovernorStatusStrip({ governor }: GovernorStatusStripProps) {
  const [expanded, setExpanded] = useState(true);
  const wanPct =
    governor.wan_cap_mbps > 0
      ? (governor.wan_rx_mbps / governor.wan_cap_mbps) * 100
      : 0;

  const stageLabel =
    govStageLabel(governor.degradation_stage);

  function govStageLabel(stage: number): string {
    if (stage === 2) return "Overloaded (Audio-Only)";
    if (stage === 1) return "High Load (Slow Preview)";
    return "Nominal";
  }

  const stageBadgeClass =
    governor.degradation_stage === 2
      ? "border-[var(--negative)]/50 bg-[var(--negative-soft)] text-[var(--negative-text)]"
      : governor.degradation_stage === 1
      ? "border-[var(--warning)]/50 bg-[var(--warning-soft)] text-[var(--warning-text)]"
      : "border-[var(--positive)]/40 bg-[var(--positive-soft)] text-[var(--positive-text)]";

  return (
    <section
      aria-labelledby="governor-telemetry-heading"
      className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] px-3 py-2"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2.5 min-w-0">
          <h2
            id="governor-telemetry-heading"
            className="flex items-center gap-1.5 text-xs font-semibold text-[var(--text-secondary)]"
          >
            <Cpu size={14} strokeWidth={1.5} className="text-[var(--text-muted)]" aria-hidden="true" />
            <span>System & Auto-Protection Status</span>
          </h2>
          <span
            className="truncate text-xs font-mono-tabular text-[var(--text-primary)]"
            title={governor.gpu_adapter_name}
          >
            {governor.gpu_adapter_name}
          </span>
          <InfoTip entry={ANALYTICS_GLOSSARY.governorStage} focusable>
            <span className={`inline-flex items-center gap-1 rounded border px-2 py-0.5 text-[11px] font-mono-tabular ${stageBadgeClass}`}>
              <Activity size={11} strokeWidth={1.5} aria-hidden="true" />
              <span>{stageLabel}</span>
            </span>
          </InfoTip>
          <InfoTip entry={ANALYTICS_GLOSSARY.activeWorkers} underline focusable>
            <span className="rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-0.5 text-[11px] font-mono-tabular text-[var(--text-secondary)]">
              {governor.active_workers} worker{governor.active_workers === 1 ? "" : "s"}
            </span>
          </InfoTip>
          {governor.shed_channels > 0 && (
            <InfoTip entry={ANALYTICS_GLOSSARY.shedChannels} focusable>
              <span className="inline-flex items-center gap-1 rounded border border-[var(--warning)]/50 bg-[var(--warning-soft)] px-2 py-0.5 text-[11px] font-mono-tabular text-[var(--warning-text)]">
                <ShieldAlert size={11} strokeWidth={1.5} aria-hidden="true" />
                <span>{governor.shed_channels} downgraded</span>
              </span>
            </InfoTip>
          )}
          {governor.on_battery && (
            <span className="inline-flex items-center gap-1 rounded border border-[var(--warning)]/50 bg-[var(--warning-soft)] px-2 py-0.5 text-[11px] font-medium text-[var(--warning-text)]">
              <BatteryWarning size={12} strokeWidth={1.5} aria-hidden="true" />
              <span>On Battery Power</span>
            </span>
          )}
        </div>

        <button
          type="button"
          onClick={() => setExpanded((prev) => !prev)}
          aria-expanded={expanded}
          aria-controls="governor-meters-grid"
          className="inline-flex h-7 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
        >
          <span>{expanded ? "Hide Meters" : "Show Meters"}</span>
          {expanded ? <ChevronUp size={13} strokeWidth={1.5} /> : <ChevronDown size={13} strokeWidth={1.5} />}
        </button>
      </div>

      {expanded && (
        <div
          id="governor-meters-grid"
          className="mt-2 grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-5"
        >
          <MeterCell
            label="GPU Memory (VRAM)"
            tip={ANALYTICS_GLOSSARY.vram}
            pct={governor.vram_pct}
            detail={`${governor.vram_used_mb} / ${governor.vram_budget_mb} MB`}
          />
          <MeterCell
            label="System RAM"
            tip={ANALYTICS_GLOSSARY.ram}
            pct={governor.ram_pct}
            detail={`${governor.ram_used_mb} / ${governor.ram_total_mb} MB`}
          />
          <MeterCell
            label="Host CPU"
            tip={ANALYTICS_GLOSSARY.cpu}
            pct={governor.cpu_pct}
            detail={`${governor.active_workers} active decoders`}
          />
          <MeterCell
            label="Internet Bandwidth"
            tip={ANALYTICS_GLOSSARY.wan}
            pct={wanPct}
            detail={`${governor.wan_rx_mbps.toFixed(1)} / ${governor.wan_cap_mbps.toFixed(0)} Mb/s`}
          />
          <MeterCell
            label="Decoder Load"
            tip={ANALYTICS_GLOSSARY.maxDecodeLatency}
            pct={governor.max_decode_latency_pct}
            detail={
              governor.shed_channels > 0
                ? `${governor.shed_channels} stepped down`
                : "Decoding smoothly"
            }
          />
        </div>
      )}
    </section>
  );
}

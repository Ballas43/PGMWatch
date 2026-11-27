import { useEffect, useRef, useState } from "react";
import {
  BatteryWarning,
  Check,
  Download,
  Flag,
  Menu,
  PauseCircle,
  Pencil,
  PlayCircle,
  Square,
  X,
} from "lucide-react";
import { DashboardSnapshot } from "../types";
import { ANALYTICS_GLOSSARY } from "../utils/analyticsGlossary";
import { BrandLogo } from "./BrandLogo";
import { InfoTip } from "./InfoTip";

interface TopBarProps {
  snapshot: DashboardSnapshot | null;
  onToggleGlobalIntermission: (active: boolean) => void;
  onStartMatch: (label: string) => void;
  onEndMatch: () => void;
  onOpenMobileMenu: () => void;
  onRenameShow: (title: string) => Promise<void> | void;
}

export function TopBar({
  snapshot,
  onToggleGlobalIntermission,
  onStartMatch,
  onEndMatch,
  onOpenMobileMenu,
  onRenameShow,
}: TopBarProps) {
  const [matchLabel, setMatchLabel] = useState("Segment 1");
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [savingTitle, setSavingTitle] = useState(false);
  const titleInputRef = useRef<HTMLInputElement>(null);
  const renameTriggerRef = useRef<HTMLButtonElement>(null);

  const gov = snapshot?.governor;
  const globalInterm = snapshot?.global_intermission ?? false;
  const activeMatch = snapshot?.active_match ?? null;
  const activeChannels = (snapshot?.channels ?? []).filter((c) => c.config.enabled).length;
  const currentTitle = snapshot?.show.title ?? "Untitled Show";

  useEffect(() => {
    if (editingTitle) {
      titleInputRef.current?.focus();
      titleInputRef.current?.select();
    }
  }, [editingTitle]);

  const startEditingTitle = () => {
    setTitleDraft(currentTitle);
    setEditingTitle(true);
  };

  const cancelEditingTitle = () => {
    setEditingTitle(false);
    window.requestAnimationFrame(() => renameTriggerRef.current?.focus());
  };

  const submitTitle = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = titleDraft.trim();
    if (!trimmed || savingTitle) return;
    if (trimmed === currentTitle) {
      cancelEditingTitle();
      return;
    }
    setSavingTitle(true);
    try {
      await onRenameShow(trimmed);
      setEditingTitle(false);
      window.requestAnimationFrame(() => renameTriggerRef.current?.focus());
    } finally {
      setSavingTitle(false);
    }
  };

  const stageLabel =
    gov?.degradation_stage === 2
      ? "Overloaded"
      : gov?.degradation_stage === 1
      ? "High Load"
      : "Nominal";

  const stageColor =
    gov?.degradation_stage === 2
      ? "text-[var(--negative-text)]"
      : gov?.degradation_stage === 1
      ? "text-[var(--warning-text)]"
      : "text-[var(--positive-text)]";

  const renderSegmentControl = (compactMobile = false) =>
    activeMatch ? (
      <button
        type="button"
        onClick={onEndMatch}
        className={`inline-flex h-8 shrink-0 items-center gap-2 whitespace-nowrap rounded-lg border border-[var(--accent)]/40 bg-[var(--accent-soft)] px-3 text-xs font-medium text-[var(--accent-text)] transition-colors hover:bg-[var(--accent)]/20 ${
          compactMobile ? "w-full justify-center" : ""
        }`}
        title="End active segment marker"
      >
        <Square size={14} strokeWidth={1.5} />
        <span className="font-mono-tabular">{activeMatch.label}</span>
      </button>
    ) : (
      <div className={`flex shrink-0 items-center gap-1.5 ${compactMobile ? "w-full" : ""}`}>
        <input
          type="text"
          aria-label="Segment marker label"
          placeholder="Segment 1"
          value={matchLabel}
          onChange={(e) => setMatchLabel(e.target.value)}
          className={`h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs text-[var(--text-primary)] ${
            compactMobile ? "min-w-0 flex-1" : "w-24"
          }`}
        />
        <InfoTip entry={ANALYTICS_GLOSSARY.segmentMarker}>
          <button
            type="button"
            onClick={() => onStartMatch(matchLabel.trim() || "Segment")}
            className="inline-flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-primary)] transition-colors hover:border-[var(--border-strong)]"
          >
            <Flag size={14} strokeWidth={1.5} className="text-[var(--text-secondary)]" />
            <span>Start Segment</span>
          </button>
        </InfoTip>
      </div>
    );

  return (
    <>
    <header className="flex h-12 shrink-0 items-center justify-between gap-2 sm:gap-3 border-b border-[var(--border-hairline)] bg-[var(--bg-rail)] px-3">
      <div className="flex items-center gap-2.5 sm:gap-3 min-w-0">
        <button
          type="button"
          onClick={onOpenMobileMenu}
          aria-label="Open navigation menu"
          className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)] lg:hidden"
        >
          <Menu size={16} strokeWidth={1.5} />
        </button>

        <BrandLogo size={26} variant="badge" className="lg:hidden" />

        <div className="flex min-w-0 items-center gap-2.5">
          {editingTitle ? (
            <form onSubmit={(e) => void submitTitle(e)} className="flex items-center gap-1.5 min-w-0">
              <input
                ref={titleInputRef}
                type="text"
                required
                maxLength={160}
                aria-label="Show title"
                placeholder="Enter show title"
                value={titleDraft}
                onChange={(e) => setTitleDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Escape") {
                    e.preventDefault();
                    cancelEditingTitle();
                  }
                }}
                className="h-8 w-44 sm:w-64 rounded-lg border border-[var(--accent)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
              />
              <button
                type="submit"
                disabled={savingTitle || !titleDraft.trim()}
                aria-label="Save show title"
                className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-[var(--positive)]/50 bg-[var(--positive-soft)] text-[var(--positive-text)] hover:bg-[var(--positive)]/25 disabled:opacity-50"
              >
                <Check size={14} strokeWidth={1.75} />
              </button>
              <button
                type="button"
                onClick={cancelEditingTitle}
                aria-label="Cancel renaming show"
                className="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              >
                <X size={14} strokeWidth={1.5} />
              </button>
            </form>
          ) : (
            <button
              ref={renameTriggerRef}
              type="button"
              onClick={startEditingTitle}
              aria-label={`Active show: ${currentTitle}. Click to rename`}
              title="Click to rename active Show Title (updates header & Excel exports without restarting feeds)"
              className="group/show flex min-w-0 max-w-44 2xl:max-w-64 items-center gap-2 overflow-hidden rounded-lg border border-transparent px-1.5 py-1 text-left transition-colors hover:border-[var(--border-hairline)] hover:bg-[var(--bg-page)]"
            >
              <span
                className={`h-2 w-2 shrink-0 rounded-full ${
                  activeChannels > 0 ? "bg-[var(--brand-tally)]" : "bg-[var(--text-muted)]/50"
                }`}
                aria-hidden="true"
              />
              <span className="hidden sm:inline shrink-0 rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--text-muted)]">
                Show
              </span>
              <span className="min-w-0 truncate text-sm font-medium text-[var(--text-primary)]">
                {currentTitle}
              </span>
              <Pencil
                size={12}
                strokeWidth={1.5}
                aria-hidden="true"
                className="shrink-0 text-[var(--text-muted)] opacity-55 transition-opacity group-hover/show:opacity-100"
              />
            </button>
          )}

          <div className="hidden md:flex shrink-0 items-center gap-2 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2.5 py-1 text-[11px] font-mono-tabular whitespace-nowrap">
            <InfoTip entry={ANALYTICS_GLOSSARY.ccv} focusable>
              <span className="text-[var(--text-muted)]">
                Viewers <span className="font-medium text-[var(--text-primary)]">{(snapshot?.combined_ccv ?? 0).toLocaleString()}</span>
              </span>
            </InfoTip>
            <span className="text-[var(--border-strong)]" aria-hidden="true">/</span>
            <InfoTip entry={ANALYTICS_GLOSSARY.pcv} focusable>
              <span className="text-[var(--text-muted)]">
                Peak <span className="font-medium text-[var(--text-primary)]">{(snapshot?.combined_pcv ?? 0).toLocaleString()}</span>
              </span>
            </InfoTip>
            <span className="text-[var(--border-strong)]" aria-hidden="true">/</span>
            <InfoTip entry={ANALYTICS_GLOSSARY.hw} focusable>
              <span className="text-[var(--text-muted)]">
                <span className="hidden 2xl:inline">Watch Hrs </span>
                <span className="font-medium text-[var(--text-primary)]">{(snapshot?.hours_watched ?? 0).toFixed(1)}h</span>
              </span>
            </InfoTip>
          </div>
        </div>
      </div>

      {/* Governor Strip */}
      <div className="hidden xl:flex shrink-0 items-center gap-2.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2.5 py-1.5 text-xs font-mono-tabular whitespace-nowrap">
        <span className="hidden 2xl:inline text-[var(--text-secondary)] truncate max-w-40" title={gov?.gpu_adapter_name}>
          {gov?.gpu_adapter_name ?? "GPU Probe"}
        </span>
        <InfoTip entry={ANALYTICS_GLOSSARY.vram} focusable>
          <span className="text-[var(--text-muted)]">
            GPU <span className="text-[var(--text-primary)]">{(gov?.vram_pct ?? 0).toFixed(0)}%</span>
          </span>
        </InfoTip>
        <InfoTip entry={ANALYTICS_GLOSSARY.ram} focusable>
          <span className="text-[var(--text-muted)]">
            RAM <span className="text-[var(--text-primary)]">{(gov?.ram_pct ?? 0).toFixed(0)}%</span>
          </span>
        </InfoTip>
        <InfoTip entry={ANALYTICS_GLOSSARY.cpu} focusable>
          <span className="text-[var(--text-muted)]">
            CPU <span className="text-[var(--text-primary)]">{(gov?.cpu_pct ?? 0).toFixed(0)}%</span>
          </span>
        </InfoTip>
        <InfoTip entry={ANALYTICS_GLOSSARY.wan} focusable>
          <span className="hidden 2xl:inline text-[var(--text-muted)]">
            Net{" "}
            <span className="text-[var(--text-primary)]">
              {(gov?.wan_rx_mbps ?? 0).toFixed(1)}/{(gov?.wan_cap_mbps ?? 250).toFixed(0)}M
            </span>
          </span>
        </InfoTip>
        <InfoTip entry={ANALYTICS_GLOSSARY.maxDecodeLatency} focusable>
          <span className="hidden 2xl:inline text-[var(--text-muted)]">
            Lag <span className="text-[var(--text-primary)]">{(gov?.max_decode_latency_pct ?? 0).toFixed(0)}%</span>
          </span>
        </InfoTip>
        {gov?.on_battery && (
          <span
            className="inline-flex items-center gap-1 rounded bg-[var(--warning-soft)] px-1.5 py-0.5 text-[11px] text-[var(--warning-text)]"
            title="Host running on battery power"
          >
            <BatteryWarning size={11} strokeWidth={1.5} aria-hidden="true" />
            <span>Battery</span>
          </span>
        )}
        <InfoTip entry={ANALYTICS_GLOSSARY.governorStage} focusable>
          <span className={stageColor}>{stageLabel}</span>
        </InfoTip>
      </div>

      {/* Operator Global Controls */}
      <div className="flex shrink-0 items-center gap-2 whitespace-nowrap">
        <div className="hidden sm:flex shrink-0 items-center">
          {renderSegmentControl(false)}
        </div>

        <InfoTip entry={ANALYTICS_GLOSSARY.holdMode}>
          <button
            type="button"
            onClick={() => onToggleGlobalIntermission(!globalInterm)}
            aria-pressed={globalInterm}
            className={`inline-flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg border px-3 text-xs font-medium transition-colors ${
              globalInterm
                ? "border-[var(--warning)]/50 bg-[var(--warning-soft)] text-[var(--warning-text)]"
                : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            }`}
          >
            {globalInterm ? <PauseCircle size={14} strokeWidth={1.5} /> : <PlayCircle size={14} strokeWidth={1.5} />}
            <span>{globalInterm ? "Hold Active" : "Hold Mode"}</span>
          </button>
        </InfoTip>

        <a
          href="/api/export.xlsx"
          className="inline-flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-lg bg-[var(--text-primary)] px-3 text-xs font-medium text-[var(--bg-rail)] transition-[transform,opacity] hover:opacity-90 active:scale-[0.98]"
        >
          <Download size={14} strokeWidth={1.5} />
          <span className="hidden md:inline">Export .xlsx</span>
        </a>
      </div>
    </header>
    <div className="flex sm:hidden shrink-0 items-center gap-2 border-b border-[var(--border-hairline)] bg-[var(--bg-rail)] px-3 py-2">
      {renderSegmentControl(true)}
    </div>
    </>
  );
}

import { useEffect, useState } from "react";
import { CheckSquare, Layers } from "lucide-react";
import type { FeedTier, LanguageGroupConfig } from "../../types";

export interface BulkChannelPatch {
  group_id?: string;
  tier?: FeedTier;
  orientation_vertical?: boolean;
  target_lufs?: number | null;
  broadcast_delay_secs?: number;
  exclude_from_accounting?: boolean;
  live_streams_only?: boolean;
  enabled?: boolean;
}

interface BulkChannelEditBarProps {
  groups: LanguageGroupConfig[];
  selectedCount: number;
  totalCount: number;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onSelectByGroup: (groupId: string) => void;
  onApplyBulk: (patch: BulkChannelPatch) => Promise<boolean>;
}

const KEEP = "__keep__";

export function BulkChannelEditBar({
  groups,
  selectedCount,
  totalCount,
  onSelectAll,
  onClearSelection,
  onSelectByGroup,
  onApplyBulk,
}: BulkChannelEditBarProps) {
  const [groupVal, setGroupVal] = useState<string>(KEEP);
  const [tierVal, setTierVal] = useState<string>(KEEP);
  const [canvasVal, setCanvasVal] = useState<string>(KEEP);
  const [lufsMode, setLufsMode] = useState<string>(KEEP);
  const [lufsCustom, setLufsCustom] = useState<string>("-14");
  const [delayMode, setDelayMode] = useState<string>(KEEP);
  const [delayCustom, setDelayCustom] = useState<string>("0");
  const [excludeVal, setExcludeVal] = useState<string>(KEEP);
  const [liveOnlyVal, setLiveOnlyVal] = useState<string>(KEEP);
  const [stateVal, setStateVal] = useState<string>(KEEP);
  const [applying, setApplying] = useState(false);

  // Clear stale category selection if the chosen group is removed or show preset switches
  useEffect(() => {
    if (groupVal !== KEEP && !groups.some((g) => g.id === groupVal)) {
      setGroupVal(KEEP);
    }
  }, [groups, groupVal]);

  const resetFields = () => {
    setGroupVal(KEEP);
    setTierVal(KEEP);
    setCanvasVal(KEEP);
    setLufsMode(KEEP);
    setDelayMode(KEEP);
    setExcludeVal(KEEP);
    setLiveOnlyVal(KEEP);
    setStateVal(KEEP);
  };

  const buildPatch = (): BulkChannelPatch => {
    const patch: BulkChannelPatch = {};
    if (groupVal !== KEEP) patch.group_id = groupVal;
    if (tierVal !== KEEP) patch.tier = tierVal as FeedTier;
    if (canvasVal !== KEEP) patch.orientation_vertical = canvasVal === "vertical";
    if (lufsMode === "auto") {
      patch.target_lufs = null;
    } else if (lufsMode === "custom") {
      const trimmed = lufsCustom.trim();
      if (trimmed !== "") {
        const parsed = Number(trimmed);
        if (Number.isFinite(parsed)) {
          patch.target_lufs = Math.max(-36, Math.min(-5, parsed));
        }
      }
    }
    if (delayMode === "set") {
      const parsed = Math.max(0, Math.min(7200, Math.round(Number(delayCustom) || 0)));
      patch.broadcast_delay_secs = parsed;
    }
    if (excludeVal !== KEEP) patch.exclude_from_accounting = excludeVal === "exclude";
    if (liveOnlyVal !== KEEP) patch.live_streams_only = liveOnlyVal === "live";
    if (stateVal !== KEEP) patch.enabled = stateVal === "active";
    return patch;
  };

  const hasChanges = Object.keys(buildPatch()).length > 0;

  const handleApply = async () => {
    const patch = buildPatch();
    if (selectedCount === 0 || Object.keys(patch).length === 0) return;
    setApplying(true);
    try {
      const ok = await onApplyBulk(patch);
      if (ok) {
        resetFields();
        onClearSelection();
      }
    } finally {
      setApplying(false);
    }
  };

  const selectCls =
    "h-7 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-primary)]";

  return (
    <div className="mb-3 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)]/45 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span className="inline-flex items-center gap-1.5 font-medium text-[var(--text-primary)]">
            <Layers size={13} strokeWidth={1.5} className="text-[var(--text-muted)]" />
            <span>Bulk Edit Feeds</span>
          </span>
          <span className="font-mono-tabular text-[11px] text-[var(--text-muted)]">
            {selectedCount} of {totalCount} selected
          </span>
          <button
            type="button"
            disabled={totalCount === 0 || selectedCount === totalCount}
            onClick={onSelectAll}
            className="inline-flex h-6 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2 text-[11px] text-[var(--text-secondary)] hover:text-[var(--text-primary)] disabled:opacity-45"
          >
            <CheckSquare size={11} strokeWidth={1.5} />
            <span>Select All</span>
          </button>
          {groups.length > 0 && (
            <select
              aria-label="Select all feeds in category"
              value=""
              onChange={(e) => {
                if (e.target.value) onSelectByGroup(e.target.value);
              }}
              className="h-6 rounded border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2 text-[11px] text-[var(--text-secondary)]"
            >
              <option value="">Select Category…</option>
              {groups.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.language_code} — {g.name}
                </option>
              ))}
            </select>
          )}
          {selectedCount > 0 && (
            <button
              type="button"
              onClick={onClearSelection}
              className="h-6 rounded border border-[var(--border-hairline)] bg-[var(--bg-card)] px-2 text-[11px] text-[var(--text-muted)] hover:text-[var(--text-primary)]"
            >
              Clear
            </button>
          )}
        </div>
        {selectedCount === 0 && (
          <span className="text-[11px] text-[var(--text-muted)]">
            Check multiple feeds below to change their category, tier, LUFS, delay, or flags all at once.
          </span>
        )}
      </div>

      {selectedCount > 0 && (
        <div className="mt-2.5 flex flex-wrap items-end gap-2.5 border-t border-[var(--border-hairline)] pt-2.5 text-xs">
          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Category</span>
            <select
              aria-label="Bulk category"
              value={groupVal}
              onChange={(e) => setGroupVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              {groups.map((g) => (
                <option key={g.id} value={g.id}>
                  {g.language_code} — {g.name}
                </option>
              ))}
            </select>
          </label>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Tier</span>
            <select
              aria-label="Bulk tier"
              value={tierVal}
              onChange={(e) => setTierVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              <option value="MASTER">MASTER</option>
              <option value="SIMULCAST">SIMULCAST</option>
              <option value="LIGHT">LIGHT</option>
            </select>
          </label>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Canvas</span>
            <select
              aria-label="Bulk canvas orientation"
              value={canvasVal}
              onChange={(e) => setCanvasVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              <option value="landscape">16:9 Landscape</option>
              <option value="vertical">9:16 Vertical</option>
            </select>
          </label>

          <div className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">LUFS Target</span>
            <div className="flex items-center gap-1">
              <select
                aria-label="Bulk LUFS target mode"
                value={lufsMode}
                onChange={(e) => setLufsMode(e.target.value)}
                className={selectCls}
              >
                <option value={KEEP}>— Keep —</option>
                <option value="auto">Auto</option>
                <option value="custom">Custom</option>
              </select>
              {lufsMode === "custom" && (
                <input
                  aria-label="Bulk custom LUFS value"
                  type="number"
                  step="0.5"
                  min={-36}
                  max={-5}
                  value={lufsCustom}
                  onChange={(e) => setLufsCustom(e.target.value)}
                  className="h-7 w-16 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-xs text-[var(--text-primary)]"
                />
              )}
            </div>
          </div>

          <div className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Delay (s)</span>
            <div className="flex items-center gap-1">
              <select
                aria-label="Bulk delay mode"
                value={delayMode}
                onChange={(e) => setDelayMode(e.target.value)}
                className={selectCls}
              >
                <option value={KEEP}>— Keep —</option>
                <option value="set">Set</option>
              </select>
              {delayMode === "set" && (
                <input
                  aria-label="Bulk delay seconds"
                  type="number"
                  min={0}
                  max={7200}
                  step={30}
                  value={delayCustom}
                  onChange={(e) => setDelayCustom(e.target.value)}
                  className="h-7 w-16 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-xs text-[var(--text-primary)]"
                />
              )}
            </div>
          </div>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Accounting</span>
            <select
              aria-label="Bulk exclude stats"
              value={excludeVal}
              onChange={(e) => setExcludeVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              <option value="include">Include</option>
              <option value="exclude">Exclude</option>
            </select>
          </label>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">Live Only</span>
            <select
              aria-label="Bulk live only"
              value={liveOnlyVal}
              onChange={(e) => setLiveOnlyVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              <option value="live">Live Only</option>
              <option value="vod">Allow VOD</option>
            </select>
          </label>

          <label className="flex flex-col gap-1">
            <span className="text-[11px] text-[var(--text-muted)]">State</span>
            <select
              aria-label="Bulk feed state"
              value={stateVal}
              onChange={(e) => setStateVal(e.target.value)}
              className={selectCls}
            >
              <option value={KEEP}>— Keep —</option>
              <option value="active">Active</option>
              <option value="paused">Paused</option>
            </select>
          </label>

          <button
            type="button"
            disabled={!hasChanges || applying}
            onClick={() => void handleApply()}
            className="h-7 rounded-lg bg-[var(--text-primary)] px-3 text-xs font-medium text-[var(--bg-rail)] transition-opacity hover:opacity-90 disabled:cursor-default disabled:opacity-40"
          >
            {applying ? "Applying…" : `Apply to ${selectedCount} Feed${selectedCount === 1 ? "" : "s"}`}
          </button>
        </div>
      )}
    </div>
  );
}

import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Plus, Trash2 } from "lucide-react";
import { ANALYTICS_GLOSSARY } from "../../utils/analyticsGlossary";
import { ChannelConfig, FeedTier, LanguageGroupConfig } from "../../types";
import { InfoTip } from "../InfoTip";
import { BulkChannelEditBar, type BulkChannelPatch } from "./BulkChannelEditBar";

interface ChannelsSectionProps {
  groups: LanguageGroupConfig[];
  showChannels: ChannelConfig[];
  channelDrafts: Record<string, ChannelConfig>;
  dirtyChannels: Record<string, boolean>;
  name: string;
  url: string;
  groupId: string;
  tier: FeedTier;
  vertical: boolean;
  delaySecs: number;
  excludeFromStats: boolean;
  liveOnly: boolean;
  onNameChange: (v: string) => void;
  onUrlChange: (v: string) => void;
  onGroupIdChange: (v: string) => void;
  onTierChange: (v: FeedTier) => void;
  onVerticalChange: (v: boolean) => void;
  onDelaySecsChange: (v: number) => void;
  onExcludeFromStatsChange: (v: boolean) => void;
  onLiveOnlyChange: (v: boolean) => void;
  onInjectSimChannel: (mode: "dead_air" | "frozen" | "clipping" | "hum") => void;
  onAddChannel: (e: React.FormEvent) => void;
  onUpdateChannelDraft: (id: string, patch: Partial<ChannelConfig>) => void;
  onSaveChannelRow: (id: string) => void;
  onToggleChannelEnabled: (ch: ChannelConfig) => void;
  onRemoveChannel: (id: string) => void;
  onChangeTier: (id: string, nextTier: FeedTier) => void;
  onBulkUpdateChannels: (ids: string[], patch: BulkChannelPatch) => Promise<boolean>;
}

const FEED_TIER_GUIDE: Array<{
  id: FeedTier;
  badge: string;
  headline: string;
  detail: string;
}> = [
  {
    id: "MASTER",
    badge: "Priority 1 · Primary Feed",
    headline: "Full source video + audio reference",
    detail:
      "Pulls best available resolution, sets the group's audio-match reference, and runs full A/V fault & freeze checks (steps down to 480p first if bandwidth gets tight).",
  },
  {
    id: "SIMULCAST",
    badge: "Priority 2 · Auto-Managed",
    headline: "<=480p video + audio vs Master",
    detail:
      "Saves bandwidth by decoding up to 480p with full freeze, black, clip & LUFS checks. Auto-steps down to Audio-Only under heavy GPU or bandwidth pressure.",
  },
  {
    id: "LIGHT",
    badge: "Priority 3 · 0% GPU Decode",
    headline: "Audio-only DSP + network & stats",
    detail:
      "Skips video decoding completely to save GPU/VRAM while still checking LUFS, dead air, clipping, Wrong Bus, stalls, live viewers, and live chat.",
  },
];

export function ChannelsSection({
  groups,
  showChannels,
  channelDrafts,
  dirtyChannels,
  name,
  url,
  groupId,
  tier,
  vertical,
  delaySecs,
  excludeFromStats,
  liveOnly,
  onNameChange,
  onUrlChange,
  onGroupIdChange,
  onTierChange,
  onVerticalChange,
  onDelaySecsChange,
  onExcludeFromStatsChange,
  onLiveOnlyChange,
  onInjectSimChannel,
  onAddChannel,
  onUpdateChannelDraft,
  onSaveChannelRow,
  onToggleChannelEnabled,
  onRemoveChannel,
  onChangeTier,
  onBulkUpdateChannels,
}: ChannelsSectionProps) {
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  // Prune any deleted channel IDs from the selection
  useEffect(() => {
    const valid = new Set(showChannels.map((c) => c.id));
    setSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (valid.has(id)) next.add(id);
        else changed = true;
      }
      return changed ? next : prev;
    });
  }, [showChannels]);

  const allSelected = showChannels.length > 0 && selectedIds.size === showChannels.length;
  const toggleSelected = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleSelectAll = () => {
    if (allSelected) setSelectedIds(new Set());
    else setSelectedIds(new Set(showChannels.map((c) => c.id)));
  };
  const selectByGroup = (gid: string) => {
    setSelectedIds(new Set(showChannels.filter((c) => c.group_id === gid).map((c) => c.id)));
  };

  const normalizeStreamUrl = (raw: string) => raw.trim().toLowerCase().replace(/\/+$/, "");

  const { duplicateGroups, duplicateChannelIds, addUrlMatches } = useMemo(() => {
    const byNorm = new Map<string, { url: string; names: string[]; ids: string[] }>();
    for (const ch of showChannels) {
      const draft = channelDrafts[ch.id] ?? ch;
      const norm = normalizeStreamUrl(draft.url);
      if (!norm || norm.startsWith("sim://")) continue;
      const entry = byNorm.get(norm) ?? { url: draft.url.trim(), names: [], ids: [] };
      entry.names.push(draft.name.trim() || ch.name);
      entry.ids.push(ch.id);
      byNorm.set(norm, entry);
    }
    const groupsList: { url: string; names: string[] }[] = [];
    const dupIds = new Set<string>();
    for (const entry of byNorm.values()) {
      if (entry.ids.length > 1) {
        groupsList.push({ url: entry.url, names: entry.names });
        for (const id of entry.ids) dupIds.add(id);
      }
    }
    const typedNorm = normalizeStreamUrl(url);
    const matches =
      typedNorm && !typedNorm.startsWith("sim://") ? byNorm.get(typedNorm)?.names ?? [] : [];
    return { duplicateGroups: groupsList, duplicateChannelIds: dupIds, addUrlMatches: matches };
  }, [showChannels, channelDrafts, url]);

  return (
    <>
      <form
        onSubmit={onAddChannel}
        className="grid grid-cols-1 gap-3 rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 md:grid-cols-6"
      >
        <div className="md:col-span-6 flex flex-wrap items-baseline justify-between gap-2 pb-1">
          <div>
            <h2 className="text-sm font-medium text-[var(--text-primary)]">Add Channel / Feed</h2>
            <p className="text-xs text-[var(--text-muted)]">
              Add a live stream URL and pick a monitoring tier based on how much video decoding and GPU load it needs.
            </p>
          </div>
        </div>

        <div className="md:col-span-2">
          <label htmlFor="ch-name" className="mb-1 block text-xs text-[var(--text-muted)]">
            Channel Name
          </label>
          <input
            id="ch-name"
            type="text"
            required
            maxLength={120}
            value={name}
            onChange={(e) => onNameChange(e.target.value)}
            placeholder="ID YouTube Backup"
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
          />
        </div>

        <div className="md:col-span-2">
          <label htmlFor="ch-url" className="mb-1 block text-xs text-[var(--text-muted)]">
            Stream URL (Platform Link or RTMP / SRT / HLS)
          </label>
          <input
            id="ch-url"
            type="text"
            required
            maxLength={2048}
            value={url}
            onChange={(e) => onUrlChange(e.target.value)}
            placeholder="https://youtube.com/@channel/live or rtmp://... / srt://..."
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm font-mono-tabular text-[var(--text-primary)]"
          />
        </div>

        <div>
          <label htmlFor="ch-group" className="mb-1 block text-xs text-[var(--text-muted)]">
            Stream Group
          </label>
          <select
            id="ch-group"
            value={groupId}
            onChange={(e) => onGroupIdChange(e.target.value)}
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
          >
            {groups.map((g) => (
              <option key={g.id} value={g.id}>
                {g.language_code} — {g.name}
              </option>
            ))}
          </select>
        </div>

        <div>
          <label htmlFor="ch-tier" className="mb-1 block text-xs text-[var(--text-muted)]">
            Feed Tier
          </label>
          <select
            id="ch-tier"
            value={tier}
            onChange={(e) => onTierChange(e.target.value as FeedTier)}
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
          >
            <option value="MASTER">MASTER — Full + Ref Bus</option>
            <option value="SIMULCAST">SIMULCAST — 480p Video+Audio</option>
            <option value="LIGHT">LIGHT — Audio Only (0% GPU)</option>
          </select>
        </div>

        <div
          role="group"
          aria-label="Monitoring tier comparison"
          className="md:col-span-6 grid grid-cols-1 gap-2.5 pt-1 md:grid-cols-3"
        >
          {FEED_TIER_GUIDE.map((item) => {
            const selected = tier === item.id;
            return (
              <button
                key={item.id}
                type="button"
                aria-pressed={selected}
                onClick={() => onTierChange(item.id)}
                className={`rounded-lg border p-3 text-left transition-colors ${
                  selected
                    ? "border-[var(--accent)]/60 bg-[var(--accent-soft)]/50"
                    : "border-[var(--border-hairline)] bg-[var(--bg-raised)]/45 hover:border-[var(--border-strong)]"
                }`}
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-mono-tabular text-xs font-semibold text-[var(--text-primary)]">
                    {item.id}
                  </span>
                  <span className="font-mono-tabular text-[11px] text-[var(--text-muted)]">
                    {item.badge}
                  </span>
                </div>
                <div className="mt-1 text-xs font-medium text-[var(--text-secondary)]">
                  {item.headline}
                </div>
                <p className="mt-0.5 text-[11px] leading-relaxed text-[var(--text-muted)]">
                  {item.detail}
                </p>
              </button>
            );
          })}
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 md:col-span-6 pt-1">
          <div className="flex flex-wrap items-center gap-3 text-xs">
            <label className="inline-flex items-center gap-2 text-[var(--text-secondary)]">
              <input
                type="checkbox"
                checked={vertical}
                onChange={(e) => onVerticalChange(e.target.checked)}
              />
              <span>Vertical 9:16 canvas</span>
            </label>
            <label className="inline-flex items-center gap-1.5 text-[var(--text-secondary)]">
              <InfoTip entry={ANALYTICS_GLOSSARY.channelDelay} underline focusable>
                <span>Delay (s)</span>
              </InfoTip>
              <input
                type="number"
                min={0}
                max={7200}
                step={30}
                value={delaySecs}
                onChange={(e) => onDelaySecsChange(Math.max(0, Number(e.target.value) || 0))}
                title="Leave 0 for normal 5-25s HLS/encoding latency; set 300-900s only for 5-15 min intentional client delays."
                className="h-7 w-20 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-[var(--text-primary)]"
              />
              <span className="text-[11px] text-[var(--text-muted)]">(0 = normal latency; 300–900 = 5–15m delay)</span>
            </label>
            <label className="inline-flex items-center gap-2 text-[var(--text-secondary)]">
              <input
                type="checkbox"
                checked={excludeFromStats}
                onChange={(e) => onExcludeFromStatsChange(e.target.checked)}
              />
              <InfoTip entry={ANALYTICS_GLOSSARY.excludeStats} underline focusable>
                <span>Exclude from viewer totals & Excel</span>
              </InfoTip>
            </label>
            <label className="inline-flex items-center gap-2 text-[var(--text-secondary)]">
              <input
                type="checkbox"
                checked={liveOnly}
                onChange={(e) => onLiveOnlyChange(e.target.checked)}
              />
              <InfoTip entry={ANALYTICS_GLOSSARY.liveStreamsOnly} underline focusable>
                <span>Live only (reject VOD/replay)</span>
              </InfoTip>
            </label>
            <span className="text-[var(--text-muted)]">· Fault test fill:</span>
            {(["dead_air", "frozen", "clipping", "hum"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => onInjectSimChannel(m)}
                className="rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 py-1 font-mono-tabular text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              >
                sim://{m}
              </button>
            ))}
          </div>

          <button
            type="submit"
            className="inline-flex h-9 items-center gap-1.5 rounded-lg bg-[var(--text-primary)] px-4 text-sm font-medium text-[var(--bg-rail)]"
          >
            <Plus size={16} strokeWidth={1.5} />
            <span>Add Feed</span>
          </button>
        </div>
        {addUrlMatches.length > 0 && (
          <div
            role="alert"
            className="md:col-span-6 flex items-center gap-2 rounded-lg border border-[var(--warning)]/50 bg-[var(--warning-soft)] px-3 py-2 text-xs text-[var(--warning-text)]"
          >
            <AlertTriangle size={14} strokeWidth={1.75} className="shrink-0" aria-hidden="true" />
            <span>
              <strong>Duplicate URL:</strong> This stream URL is already configured on{" "}
              <span className="font-medium">{addUrlMatches.join(", ")}</span>. Adding it again will
              double bandwidth and double-count viewers in accounting.
            </span>
          </div>
        )}
      </form>

      <div className="overflow-x-auto rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-4">
        {duplicateGroups.length > 0 && (
          <div
            role="alert"
            className="mb-3 rounded-lg border border-[var(--warning)]/55 bg-[var(--warning-soft)] px-3.5 py-2.5 text-xs text-[var(--warning-text)]"
          >
            <div className="flex items-center gap-2 font-medium">
              <AlertTriangle size={14} strokeWidth={1.75} className="shrink-0" aria-hidden="true" />
              <span>
                Duplicate Stream URL{duplicateGroups.length > 1 ? "s" : ""} Detected (
                {duplicateGroups.length})
              </span>
            </div>
            <p className="mt-1 text-[11px] opacity-90">
              Monitoring the same stream URL on multiple channels doubles bandwidth usage and
              double-counts viewers in accounting. Remove or update the duplicate rows highlighted
              below:
            </p>
            <ul className="mt-1.5 space-y-0.5 font-mono-tabular text-[11px]">
              {duplicateGroups.map((dup) => (
                <li key={dup.url} className="truncate">
                  • <span className="font-sans font-medium">{dup.names.join(" & ")}</span> —{" "}
                  {dup.url}
                </li>
              ))}
            </ul>
          </div>
        )}
        <BulkChannelEditBar
          groups={groups}
          selectedCount={selectedIds.size}
          totalCount={showChannels.length}
          onSelectAll={() => setSelectedIds(new Set(showChannels.map((c) => c.id)))}
          onClearSelection={() => setSelectedIds(new Set())}
          onSelectByGroup={selectByGroup}
          onApplyBulk={(patch) => onBulkUpdateChannels(Array.from(selectedIds), patch)}
        />
        <table className="w-full text-left text-xs">
          <thead>
            <tr className="border-b border-[var(--border-hairline)] text-[var(--text-muted)]">
              <th scope="col" className="pb-2.5 pr-2 w-6">
                <input
                  type="checkbox"
                  aria-label="Select all channels"
                  checked={allSelected}
                  onChange={toggleSelectAll}
                />
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">Channel Name</th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">Category</th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">Platform</th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">Stream URL</th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">9:16</th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.tier} underline focusable>Tier</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.lufsTarget} underline focusable>LUFS Target</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.lufsTarget} underline focusable>± Range</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.channelDelay} underline focusable>Delay (s)</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.excludeStats} underline focusable>Exclude Stats</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">
                <InfoTip entry={ANALYTICS_GLOSSARY.liveStreamsOnly} underline focusable>Live Only</InfoTip>
              </th>
              <th scope="col" className="pb-2.5 pr-2 font-medium">State</th>
              <th scope="col" className="pb-2.5 font-medium text-right">Actions</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-[var(--border-hairline)]">
            {showChannels.map((ch) => {
              const draft = channelDrafts[ch.id] ?? ch;
              const isDirty = Boolean(dirtyChannels[ch.id]);
              const isSelected = selectedIds.has(ch.id);
              return (
                <tr
                  key={ch.id}
                  className={
                    isSelected
                      ? "bg-[var(--accent-soft)]/35 hover:bg-[var(--accent-soft)]/50"
                      : ch.enabled
                      ? "hover:bg-[var(--bg-raised)]/40"
                      : "bg-[var(--bg-page)]/40 opacity-55 hover:bg-[var(--bg-raised)]/40"
                  }
                >
                  <td className="py-2 pr-2">
                    <input
                      type="checkbox"
                      aria-label={`Select ${ch.name} for bulk edit`}
                      checked={isSelected}
                      onChange={() => toggleSelected(ch.id)}
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <input
                      aria-label={`Name for ${ch.name}`}
                      type="text"
                      maxLength={120}
                      value={draft.name}
                      onChange={(e) => onUpdateChannelDraft(ch.id, { name: e.target.value })}
                      className="h-7 w-36 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-medium text-[var(--text-primary)]"
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <select
                      aria-label={`Category for ${ch.name}`}
                      value={draft.group_id}
                      onChange={(e) => onUpdateChannelDraft(ch.id, { group_id: e.target.value })}
                      className="h-7 max-w-[160px] rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-[var(--text-primary)]"
                    >
                      {groups.map((g) => (
                        <option key={g.id} value={g.id}>
                          {g.language_code} — {g.name}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td className="py-2 pr-2 capitalize text-[var(--text-secondary)]">
                    {draft.platform}
                  </td>
                  <td className="py-2 pr-2">
                    <div className="flex items-center gap-1.5">
                      <input
                        aria-label={`URL for ${ch.name}`}
                        type="text"
                        maxLength={2048}
                        value={draft.url}
                        onChange={(e) => onUpdateChannelDraft(ch.id, { url: e.target.value })}
                        title={
                          duplicateChannelIds.has(ch.id)
                            ? "Duplicate stream URL — shared with another channel in this show"
                            : undefined
                        }
                        className={`h-7 w-64 rounded border bg-[var(--bg-raised)] px-2 font-mono-tabular text-[var(--text-primary)] ${
                          duplicateChannelIds.has(ch.id)
                            ? "border-[var(--warning)] ring-1 ring-[var(--warning)]/40"
                            : "border-[var(--border-hairline)]"
                        }`}
                      />
                      {duplicateChannelIds.has(ch.id) && (
                        <span
                          title="This stream URL is duplicated on another channel"
                          className="inline-flex shrink-0 items-center gap-1 rounded border border-[var(--warning)]/50 bg-[var(--warning-soft)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--warning-text)]"
                        >
                          <AlertTriangle size={10} strokeWidth={2} aria-hidden="true" />
                          <span>Dup</span>
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="py-2 pr-2">
                    <input
                      aria-label={`Vertical 9:16 for ${ch.name}`}
                      type="checkbox"
                      checked={draft.orientation_vertical}
                      onChange={(e) =>
                        onUpdateChannelDraft(ch.id, { orientation_vertical: e.target.checked })
                      }
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <select
                      aria-label={`Tier for ${ch.name}`}
                      value={draft.tier}
                      onChange={(e) => onChangeTier(ch.id, e.target.value as FeedTier)}
                      className="h-7 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-[var(--text-primary)]"
                    >
                      <option value="MASTER">MASTER</option>
                      <option value="SIMULCAST">SIMULCAST</option>
                      <option value="LIGHT">LIGHT</option>
                    </select>
                  </td>
                  <td className="py-2 pr-2">
                    <ChannelLufsInput
                      ariaLabel={`Target LUFS for ${ch.name}`}
                      value={draft.target_lufs ?? null}
                      onCommit={(nextVal) =>
                        onUpdateChannelDraft(ch.id, {
                          target_lufs:
                            nextVal == null ? null : Math.max(-36, Math.min(-5, nextVal)),
                        })
                      }
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <ChannelLufsInput
                      ariaLabel={`LUFS range tolerance for ${ch.name}`}
                      value={draft.lufs_tolerance ?? null}
                      onCommit={(nextVal) =>
                        onUpdateChannelDraft(ch.id, {
                          lufs_tolerance:
                            nextVal == null ? null : Math.max(0.5, Math.min(10, nextVal)),
                        })
                      }
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <input
                      aria-label={`Broadcast delay seconds for ${ch.name}`}
                      type="number"
                      min={0}
                      max={7200}
                      step={30}
                      value={draft.broadcast_delay_secs ?? 0}
                      onChange={(e) => {
                        const parsed = Math.max(0, Math.min(7200, Math.round(Number(e.target.value) || 0)));
                        onUpdateChannelDraft(ch.id, { broadcast_delay_secs: parsed });
                      }}
                      className="h-7 w-16 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-[var(--text-primary)]"
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <input
                      aria-label={`Exclude ${ch.name} from viewer accounting`}
                      type="checkbox"
                      checked={Boolean(draft.exclude_from_accounting)}
                      onChange={(e) =>
                        onUpdateChannelDraft(ch.id, { exclude_from_accounting: e.target.checked })
                      }
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <input
                      aria-label={`Live-only check for ${ch.name}`}
                      type="checkbox"
                      checked={draft.live_streams_only ?? true}
                      onChange={(e) =>
                        onUpdateChannelDraft(ch.id, { live_streams_only: e.target.checked })
                      }
                    />
                  </td>
                  <td className="py-2 pr-2">
                    <button
                      type="button"
                      onClick={() => onToggleChannelEnabled(ch)}
                      className={`h-7 rounded border px-2 font-mono-tabular ${
                        ch.enabled
                          ? "border-[var(--positive)]/40 bg-[var(--positive-soft)] text-[var(--positive-text)]"
                          : "border-[var(--border-hairline)] bg-[var(--bg-page)] text-[var(--text-muted)]"
                      }`}
                    >
                      {ch.enabled ? "Active" : "Paused"}
                    </button>
                  </td>
                  <td className="py-2 text-right">
                    <div className="inline-flex items-center gap-1.5">
                      <button
                        type="button"
                        disabled={!isDirty}
                        onClick={() => onSaveChannelRow(ch.id)}
                        className={`h-7 rounded border px-2.5 text-xs font-medium transition-colors ${
                          isDirty
                            ? "border-[var(--positive)]/60 bg-[var(--positive-soft)] text-[var(--positive-text)] hover:bg-[var(--positive)]/25"
                            : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] opacity-45 cursor-default"
                        }`}
                      >
                        Save
                      </button>
                      <button
                        type="button"
                        aria-label={`Remove ${ch.name}`}
                        onClick={() => onRemoveChannel(ch.id)}
                        className="inline-flex h-7 w-7 items-center justify-center rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] hover:text-[#e5484d]"
                      >
                        <Trash2 size={14} strokeWidth={1.5} />
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </>
  );
}

interface ChannelLufsInputProps {
  ariaLabel: string;
  value: number | null;
  onCommit: (next: number | null) => void;
}

function ChannelLufsInput({ ariaLabel, value, onCommit }: ChannelLufsInputProps) {
  const [text, setText] = useState<string>(value == null ? "" : String(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) {
      setText(value == null ? "" : String(value));
    }
  }, [value, focused]);

  return (
    <input
      aria-label={ariaLabel}
      type="text"
      inputMode="decimal"
      placeholder="Auto"
      value={text}
      onFocus={() => setFocused(true)}
      onChange={(e) => {
        const nextText = e.target.value;
        setText(nextText);
        const trimmed = nextText.trim();
        if (trimmed === "") {
          onCommit(null);
          return;
        }
        if (trimmed === "-" || trimmed === "." || trimmed === "-.") return;
        const parsed = Number(trimmed);
        if (Number.isFinite(parsed)) {
          onCommit(parsed);
        }
      }}
      onBlur={() => {
        setFocused(false);
        const trimmed = text.trim();
        if (trimmed === "" || trimmed === "-" || trimmed === "." || trimmed === "-.") {
          setText(value == null ? "" : String(value));
          return;
        }
        const parsed = Number(trimmed);
        if (!Number.isFinite(parsed)) {
          setText(value == null ? "" : String(value));
        } else {
          onCommit(parsed);
          setText(String(parsed));
        }
      }}
      className="h-7 w-20 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular text-[var(--text-primary)]"
    />
  );
}

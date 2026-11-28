import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChannelConfig,
  DashboardSnapshot,
  FeedTier,
  LanguageGroupConfig,
  Platform,
  ShowPreset,
  ShowPresetListResponse,
  ShowPresetSummary,
} from "../types";
import { ChannelsSection } from "./show-setup/ChannelsSection";
import type { BulkChannelPatch } from "./show-setup/BulkChannelEditBar";
import { ShowPresetsCard } from "./show-setup/ShowPresetsCard";
import { ShowTitleCard } from "./show-setup/ShowTitleCard";
import { StreamGroupsCard } from "./show-setup/StreamGroupsCard";

interface ShowSetupViewProps {
  snapshot: DashboardSnapshot | null;
  onRefresh: () => void;
}

function detectPlatform(url: string): Platform {
  const trimmed = url.trim().toLowerCase();
  let host = "";
  try {
    host = new URL(trimmed.includes("://") ? trimmed : `https://${trimmed}`).hostname;
  } catch {
    host = trimmed.split("/")[0] ?? "";
  }
  const matches = (domain: string) => host === domain || host.endsWith(`.${domain}`);
  if (matches("youtube.com") || matches("youtu.be") || trimmed.startsWith("sim://yt")) return "youtube";
  if (matches("twitch.tv") || trimmed.startsWith("sim://tw")) return "twitch";
  if (matches("facebook.com") || matches("fb.watch") || matches("fb.gg") || trimmed.startsWith("sim://fb")) return "facebook";
  if (matches("tiktok.com") || trimmed.startsWith("sim://tt")) return "tiktok";
  if (matches("bigo.tv") || trimmed.startsWith("sim://bigo")) return "bigo";
  return "custom";
}

function makeSafeId(prefix: string, raw: string, existingIds: Set<string>): string {
  const slug = raw
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 24);
  const base = slug ? `${prefix}-${slug}` : `${prefix}-${Date.now().toString().slice(-4)}`;
  let candidate = base;
  let n = 2;
  while (existingIds.has(candidate)) {
    candidate = `${base}-${n}`;
    n += 1;
  }
  return candidate;
}

const EMPTY_GROUPS: LanguageGroupConfig[] = [];
const EMPTY_CHANNELS: ChannelConfig[] = [];
const DEFAULT_COMPLAINT_KEYWORDS = [
  "no sound",
  "muted",
  "bisu",
  "no audio",
  "lag",
  "freeze",
  "stutter",
  "suara hilang",
];

export function ShowSetupView({ snapshot, onRefresh }: ShowSetupViewProps) {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [groupId, setGroupId] = useState(snapshot?.show.groups[0]?.id ?? "");
  const [tier, setTier] = useState<FeedTier>("SIMULCAST");
  const [vertical, setVertical] = useState(false);
  const [delaySecs, setDelaySecs] = useState(0);
  const [excludeFromStats, setExcludeFromStats] = useState(false);
  const [liveOnly, setLiveOnly] = useState(true);

  const [newLangCode, setNewLangCode] = useState("");
  const [newLangName, setNewLangName] = useState("");

  const [groupDrafts, setGroupDrafts] = useState<Record<string, LanguageGroupConfig>>({});
  const [keywordDrafts, setKeywordDrafts] = useState<Record<string, string>>({});
  const [dirtyGroups, setDirtyGroups] = useState<Record<string, boolean>>({});

  const [showTitleDraft, setShowTitleDraft] = useState(snapshot?.show.title ?? "");
  const [dirtyShowTitle, setDirtyShowTitle] = useState(false);

  const [channelDrafts, setChannelDrafts] = useState<Record<string, ChannelConfig>>({});
  const [dirtyChannels, setDirtyChannels] = useState<Record<string, boolean>>({});

  const [presetList, setPresetList] = useState<ShowPresetSummary[]>([]);
  const [presetsLoading, setPresetsLoading] = useState(true);

  const [statusMsg, setStatusMsg] = useState<{ text: string; isError?: boolean } | null>(null);

  const activeShowId = snapshot?.show.show_id ?? "";
  const showTitleValue = snapshot?.show.title ?? "";
  const activeSegmentId = snapshot?.active_match?.id ?? "";
  const groups = snapshot?.show.groups ?? EMPTY_GROUPS;
  const showChannels = snapshot?.show.channels ?? EMPTY_CHANNELS;

  const refreshPresetList = useCallback(async () => {
    try {
      const res = await fetch("/api/shows");
      if (!res.ok) return;
      const body = (await res.json()) as ShowPresetListResponse;
      if (Array.isArray(body.shows)) {
        setPresetList(body.shows);
      }
    } finally {
      setPresetsLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshPresetList();
  }, [activeShowId, showTitleValue, groups.length, showChannels.length, activeSegmentId, refreshPresetList]);
  const groupsSig = groups
    .map(
      (g) =>
        `${g.id}\0${g.language_code}\0${g.name}\0${Boolean(g.intermission_active)}\0${g.complaint_keywords.join(",")}`
    )
    .join("\n");
  const channelsSig = showChannels
    .map(
      (c) =>
        `${c.id}\0${c.name}\0${c.group_id}\0${c.url}\0${c.tier}\0${c.orientation_vertical}\0${c.enabled}\0${c.target_lufs ?? ""}\0${c.lufs_tolerance ?? ""}\0${c.broadcast_delay_secs ?? 0}\0${Boolean(c.exclude_from_accounting)}\0${c.live_streams_only ?? true}`
    )
    .join("\n");

  useEffect(() => {
    if (!dirtyShowTitle) {
      setShowTitleDraft(showTitleValue);
    }
  }, [showTitleValue, dirtyShowTitle]);

  useEffect(() => {
    if (groups.length > 0 && !groups.some((g) => g.id === groupId)) {
      setGroupId(groups[0].id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupsSig, groupId]);

  useEffect(() => {
    setGroupDrafts((prev) => {
      const next: Record<string, LanguageGroupConfig> = {};
      for (const g of groups) {
        next[g.id] = dirtyGroups[g.id] && prev[g.id] ? prev[g.id] : g;
      }
      return next;
    });
    setKeywordDrafts((prev) => {
      const next: Record<string, string> = {};
      for (const g of groups) {
        next[g.id] =
          dirtyGroups[g.id] && prev[g.id] !== undefined
            ? prev[g.id]
            : g.complaint_keywords.join(", ");
      }
      return next;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupsSig, dirtyGroups]);

  useEffect(() => {
    setChannelDrafts((prev) => {
      const next: Record<string, ChannelConfig> = {};
      for (const ch of showChannels) {
        next[ch.id] = dirtyChannels[ch.id] && prev[ch.id] ? prev[ch.id] : ch;
      }
      return next;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channelsSig, dirtyChannels]);

  const channelCountsByGroup = useMemo(() => {
    const counts: Record<string, number> = {};
    for (const ch of showChannels) {
      counts[ch.group_id] = (counts[ch.group_id] ?? 0) + 1;
    }
    return counts;
  }, [showChannels]);

  if (!snapshot) return null;

  const savePreset = async (nextShow: ShowPreset, successText?: string): Promise<boolean> => {
    try {
      const res = await fetch("/api/show", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(nextShow),
      });
      if (res.ok) {
        setStatusMsg({ text: successText ?? "Show preset saved and persisted to disk" });
        onRefresh();
        void refreshPresetList();
        return true;
      }
      const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
      setStatusMsg({ text: `Failed to save show: ${err.error ?? res.statusText}`, isError: true });
      return false;
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error saving show",
        isError: true,
      });
      return false;
    }
  };

  const saveShowTitle = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = showTitleDraft.trim();
    if (!trimmed) {
      setStatusMsg({ text: "Show title cannot be empty", isError: true });
      return;
    }
    try {
      const res = await fetch("/api/show/title", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title: trimmed }),
      });
      if (res.ok) {
        setDirtyShowTitle(false);
        setShowTitleDraft(trimmed);
        setStatusMsg({ text: `Show renamed to "${trimmed}"` });
        onRefresh();
      } else {
        const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setStatusMsg({ text: `Failed to rename show: ${err.error ?? res.statusText}`, isError: true });
      }
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error renaming show",
        isError: true,
      });
    }
  };

  const addLanguageGroup = async (e: React.FormEvent) => {
    e.preventDefault();
    const code = newLangCode.trim().toUpperCase();
    const displayName = newLangName.trim();
    if (!code || !displayName) return;

    const existingIds = new Set(groups.map((g) => g.id));
    const id = makeSafeId("grp", `${code}-pgm`, existingIds);
    const newGroup: LanguageGroupConfig = {
      id,
      name: displayName,
      language_code: code,
      intermission_active: false,
      complaint_keywords: [...DEFAULT_COMPLAINT_KEYWORDS],
    };
    const ok = await savePreset(
      {
        ...snapshot.show,
        groups: [...groups, newGroup],
      },
      `Added stream group ${code} (${displayName})`
    );
    if (ok) {
      setNewLangCode("");
      setNewLangName("");
      setGroupId(id);
    }
  };

  const saveGroupRow = async (id: string) => {
    const draft = groupDrafts[id];
    if (!draft) return;
    const trimmedCode = draft.language_code.trim().toUpperCase();
    const trimmedName = draft.name.trim();
    if (!trimmedCode || !trimmedName) {
      setStatusMsg({ text: "Group tag and name cannot be empty", isError: true });
      return;
    }
    const rawKeywords = keywordDrafts[id] ?? draft.complaint_keywords.join(", ");
    const cleanedKeywords = Array.from(
      new Set(
        rawKeywords
          .split(",")
          .map((k) => k.trim())
          .filter(Boolean)
      )
    );
    const updated: LanguageGroupConfig = {
      ...draft,
      language_code: trimmedCode,
      name: trimmedName,
      complaint_keywords: cleanedKeywords,
    };
    const ok = await savePreset(
      {
        ...snapshot.show,
        groups: groups.map((g) => (g.id === id ? updated : g)),
      },
      `Saved stream group ${trimmedCode}`
    );
    if (ok) {
      setKeywordDrafts((prev) => ({
        ...prev,
        [id]: cleanedKeywords.join(", "),
      }));
      setDirtyGroups((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    }
  };

  const removeLanguageGroup = async (id: string) => {
    if (groups.length <= 1) {
      setStatusMsg({ text: "At least one language group must remain configured", isError: true });
      return;
    }
    if ((channelCountsByGroup[id] ?? 0) > 0) {
      setStatusMsg({
        text: "Move or remove channels in this category before deleting the language group",
        isError: true,
      });
      return;
    }
    await savePreset(
      {
        ...snapshot.show,
        groups: groups.filter((g) => g.id !== id),
      },
      "Removed language group"
    );
  };

  const addChannel = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim() || !url.trim()) return;
    const targetGroupId = groups.some((g) => g.id === groupId) ? groupId : groups[0]?.id;
    if (!targetGroupId) {
      setStatusMsg({ text: "Create a stream group first", isError: true });
      return;
    }
    const existingIds = new Set(showChannels.map((c) => c.id));
    const id = makeSafeId("ch", name, existingIds);
    const newCh: ChannelConfig = {
      id,
      group_id: targetGroupId,
      name: name.trim(),
      url: url.trim(),
      platform: detectPlatform(url),
      tier,
      priority: tier === "MASTER" ? 1 : tier === "SIMULCAST" ? 2 : 3,
      orientation_vertical: vertical,
      enabled: true,
      broadcast_delay_secs: Math.max(0, Math.min(7200, Math.round(delaySecs))),
      exclude_from_accounting: excludeFromStats,
      live_streams_only: liveOnly,
    };
    const ok = await savePreset(
      {
        ...snapshot.show,
        channels: [...showChannels, newCh],
      },
      `Added feed "${newCh.name}"`
    );
    if (ok) {
      setName("");
      setUrl("");
      setDelaySecs(0);
      setExcludeFromStats(false);
      setLiveOnly(true);
    }
  };

  const updateChannelDraft = (id: string, patch: Partial<ChannelConfig>) => {
    setDirtyChannels((prev) => ({ ...prev, [id]: true }));
    setChannelDrafts((prev) => {
      const cur = prev[id] ?? showChannels.find((c) => c.id === id);
      if (!cur) return prev;
      const merged: ChannelConfig = { ...cur, ...patch };
      if (patch.url !== undefined) {
        merged.platform = detectPlatform(patch.url);
      }
      if (patch.tier !== undefined) {
        merged.priority = patch.tier === "MASTER" ? 1 : patch.tier === "SIMULCAST" ? 2 : 3;
      }
      return { ...prev, [id]: merged };
    });
  };

  const saveChannelRow = async (id: string) => {
    const draft = channelDrafts[id];
    if (!draft) return;
    const trimmedName = draft.name.trim();
    const trimmedUrl = draft.url.trim();
    if (!trimmedName || !trimmedUrl) {
      setStatusMsg({ text: "Channel name and stream URL cannot be empty", isError: true });
      return;
    }
    const clampedLufs =
      draft.target_lufs != null && Number.isFinite(draft.target_lufs)
        ? Math.max(-36, Math.min(-5, draft.target_lufs))
        : null;
    const clampedTol =
      draft.lufs_tolerance != null && Number.isFinite(draft.lufs_tolerance)
        ? Math.max(0.5, Math.min(10, draft.lufs_tolerance))
        : null;
    const updated: ChannelConfig = {
      ...draft,
      name: trimmedName,
      url: trimmedUrl,
      target_lufs: clampedLufs,
      lufs_tolerance: clampedTol,
      platform: detectPlatform(trimmedUrl),
      priority: draft.tier === "MASTER" ? 1 : draft.tier === "SIMULCAST" ? 2 : 3,
    };
    const ok = await savePreset(
      {
        ...snapshot.show,
        channels: showChannels.map((c) => (c.id === id ? updated : c)),
      },
      `Updated feed "${trimmedName}"`
    );
    if (ok) {
      setDirtyChannels((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    }
  };

  const toggleChannelEnabled = async (ch: ChannelConfig) => {
    const nextEnabled = !ch.enabled;
    const ok = await savePreset(
      {
        ...snapshot.show,
        channels: showChannels.map((c) =>
          c.id === ch.id ? { ...c, enabled: nextEnabled } : c
        ),
      },
      `${nextEnabled ? "Activated" : "Paused"} feed "${ch.name}"`
    );
    if (ok) {
      setChannelDrafts((prev) =>
        prev[ch.id]
          ? { ...prev, [ch.id]: { ...prev[ch.id], enabled: nextEnabled } }
          : prev
      );
    }
  };

  const removeChannel = async (id: string) => {
    await savePreset(
      {
        ...snapshot.show,
        channels: showChannels.filter((c) => c.id !== id),
      },
      "Removed feed"
    );
  };

  const bulkUpdateChannels = async (ids: string[], patch: BulkChannelPatch): Promise<boolean> => {
    if (ids.length === 0 || Object.keys(patch).length === 0) return false;
    const idSet = new Set(ids);
    const updatedChannels: ChannelConfig[] = [];
    for (const c of showChannels) {
      if (!idSet.has(c.id)) {
        updatedChannels.push(c);
        continue;
      }
      // Preserve any unsaved inline edits on selected rows while applying bulk settings
      const base = channelDrafts[c.id] ?? c;
      const trimmedName = base.name.trim();
      const trimmedUrl = base.url.trim();
      if (!trimmedName || !trimmedUrl) {
        setStatusMsg({
          text: `Cannot bulk-apply: "${c.name}" has an empty unsaved name or stream URL`,
          isError: true,
        });
        return false;
      }
      const merged: ChannelConfig = {
        ...base,
        ...patch,
        name: trimmedName,
        url: trimmedUrl,
        platform: detectPlatform(trimmedUrl),
      };
      if (patch.tier !== undefined) {
        merged.priority = patch.tier === "MASTER" ? 1 : patch.tier === "SIMULCAST" ? 2 : 3;
      }
      updatedChannels.push(merged);
    }
    const ok = await savePreset(
      {
        ...snapshot.show,
        channels: updatedChannels,
      },
      `Updated ${ids.length} feed${ids.length === 1 ? "" : "s"} in bulk`
    );
    if (ok) {
      setDirtyChannels((prev) => {
        const next = { ...prev };
        for (const id of ids) delete next[id];
        return next;
      });
    }
    return ok;
  };

  const changeTier = async (id: string, nextTier: FeedTier) => {
    try {
      const res = await fetch(`/api/channels/${encodeURIComponent(id)}/tier`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ tier: nextTier }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: "Governor denied tier change" }));
        setStatusMsg({ text: err.error ?? `HTTP ${res.status}`, isError: true });
      } else {
        setChannelDrafts((prev) => {
          const cur = prev[id];
          if (!cur) return prev;
          return {
            ...prev,
            [id]: {
              ...cur,
              tier: nextTier,
              priority: nextTier === "MASTER" ? 1 : nextTier === "SIMULCAST" ? 2 : 3,
            },
          };
        });
        setStatusMsg({ text: `Updated ${id} tier to ${nextTier}` });
        onRefresh();
      }
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error updating tier",
        isError: true,
      });
    }
  };

  const resetLocalDrafts = () => {
    setDirtyShowTitle(false);
    setDirtyGroups({});
    setDirtyChannels({});
    setName("");
    setUrl("");
    setDelaySecs(0);
    setExcludeFromStats(false);
    setLiveOnly(true);
  };

  const exportPreset = (showId: string) => {
    const link = document.createElement("a");
    link.href = `/api/shows/${encodeURIComponent(showId)}/export`;
    link.download = "";
    document.body.appendChild(link);
    link.click();
    link.remove();
  };

  const switchPreset = async (showId: string) => {
    try {
      const res = await fetch(`/api/shows/${encodeURIComponent(showId)}/activate`, {
        method: "POST",
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setStatusMsg({ text: err.error ?? res.statusText, isError: true });
        return;
      }
      const nextSnap = (await res.json()) as DashboardSnapshot;
      resetLocalDrafts();
      setStatusMsg({ text: `Switched live show to "${nextSnap.show.title}"` });
      onRefresh();
      void refreshPresetList();
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error switching show preset",
        isError: true,
      });
    }
  };

  const deletePreset = async (showId: string) => {
    try {
      const res = await fetch(`/api/shows/${encodeURIComponent(showId)}/delete`, {
        method: "POST",
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setStatusMsg({ text: err.error ?? res.statusText, isError: true });
        return;
      }
      setStatusMsg({ text: "Removed saved show preset" });
      void refreshPresetList();
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error deleting show preset",
        isError: true,
      });
    }
  };

  const createNewShow = async (input: {
    title: string;
    groupCode: string;
    groupName: string;
    resetAccounting: boolean;
  }): Promise<boolean> => {
    try {
      const res = await fetch("/api/shows/create", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          title: input.title,
          group_code: input.groupCode,
          group_name: input.groupName,
          reset_accounting: input.resetAccounting,
        }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setStatusMsg({ text: err.error ?? res.statusText, isError: true });
        return false;
      }
      const nextSnap = (await res.json()) as DashboardSnapshot;
      resetLocalDrafts();
      setStatusMsg({ text: `Started new show "${nextSnap.show.title}" (active feeds replaced)` });
      onRefresh();
      void refreshPresetList();
      return true;
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? err.message : "Network error creating new show",
        isError: true,
      });
      return false;
    }
  };

  const importPresetJson = async (file: File, resetAccounting: boolean): Promise<boolean> => {
    try {
      const rawText = await file.text();
      const parsed: unknown = JSON.parse(rawText);
      const candidate =
        typeof parsed === "object" &&
        parsed !== null &&
        "preset" in parsed &&
        typeof (parsed as { preset?: unknown }).preset === "object"
          ? (parsed as { preset: ShowPreset }).preset
          : (parsed as ShowPreset);
      if (
        !candidate ||
        typeof candidate.show_id !== "string" ||
        typeof candidate.title !== "string" ||
        !Array.isArray(candidate.groups) ||
        !Array.isArray(candidate.channels)
      ) {
        setStatusMsg({
          text: "Invalid show preset file: expected PGMWatch ShowPreset JSON",
          isError: true,
        });
        return false;
      }
      const res = await fetch("/api/shows/import", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          preset: candidate,
          activate: true,
          reset_accounting: resetAccounting,
        }),
      });
      if (!res.ok) {
        const err = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
        setStatusMsg({ text: err.error ?? res.statusText, isError: true });
        return false;
      }
      const nextSnap = (await res.json()) as DashboardSnapshot;
      resetLocalDrafts();
      setStatusMsg({ text: `Imported and activated "${nextSnap.show.title}"` });
      onRefresh();
      void refreshPresetList();
      return true;
    } catch (err) {
      setStatusMsg({
        text: err instanceof Error ? `Failed importing preset: ${err.message}` : "Failed importing preset",
        isError: true,
      });
      return false;
    }
  };

  const injectSimChannel = (mode: "dead_air" | "frozen" | "clipping" | "hum") => {
    setName(`Test ${mode.replace("_", " ")}`);
    setUrl(`sim://${mode}/test-${mode}`);
  };

  return (
    <div className="space-y-4 animate-enter">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
        <h1 className="text-lg font-medium tracking-tight text-[var(--text-primary)]">
          Show Setup & Channels
        </h1>
        <p className="text-xs text-[var(--text-secondary)]">
          Manage language categories, paste public stream URLs (zero API keys), or edit live channels and tiers
        </p>
      </div>

      {statusMsg && (
        <div
          role={statusMsg.isError ? "alert" : "status"}
          aria-live="polite"
          className={`flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-xs ${
            statusMsg.isError
              ? "border-[var(--negative)]/50 bg-[var(--negative-soft)] text-[var(--negative-text)]"
              : "border-[var(--border-strong)] bg-[var(--bg-card)] text-[var(--text-primary)]"
          }`}
        >
          <span>{statusMsg.text}</span>
          <button
            type="button"
            onClick={() => setStatusMsg(null)}
            className="rounded border border-[var(--border-hairline)] px-2 py-0.5 text-[11px] hover:bg-[var(--bg-raised)]"
          >
            Dismiss
          </button>
        </div>
      )}

      <ShowPresetsCard
        activeShowId={snapshot.show.show_id}
        activeSegmentLabel={snapshot.active_match?.label ?? null}
        presets={presetList}
        loading={presetsLoading}
        onExportPreset={exportPreset}
        onSwitchPreset={switchPreset}
        onDeletePreset={deletePreset}
        onCreateNewShow={createNewShow}
        onImportPresetJson={importPresetJson}
      />

      <ShowTitleCard
        showTitleDraft={showTitleDraft}
        showTitleValue={showTitleValue}
        dirtyShowTitle={dirtyShowTitle}
        onChangeTitle={(next) => {
          setDirtyShowTitle(true);
          setShowTitleDraft(next);
        }}
        onSubmit={(e) => void saveShowTitle(e)}
      />

      <StreamGroupsCard
        groups={groups}
        groupDrafts={groupDrafts}
        keywordDrafts={keywordDrafts}
        dirtyGroups={dirtyGroups}
        channelCountsByGroup={channelCountsByGroup}
        newLangCode={newLangCode}
        newLangName={newLangName}
        onNewLangCodeChange={setNewLangCode}
        onNewLangNameChange={setNewLangName}
        onAddGroup={(e) => void addLanguageGroup(e)}
        onChangeGroupDraft={(id, next) => {
          setDirtyGroups((p) => ({ ...p, [id]: true }));
          setGroupDrafts((p) => ({ ...p, [id]: next }));
        }}
        onChangeKeywordDraft={(id, raw) => {
          setDirtyGroups((p) => ({ ...p, [id]: true }));
          setKeywordDrafts((p) => ({ ...p, [id]: raw }));
        }}
        onSaveGroup={(id) => void saveGroupRow(id)}
        onRemoveGroup={(id) => void removeLanguageGroup(id)}
      />

      <ChannelsSection
        groups={groups}
        showChannels={showChannels}
        channelDrafts={channelDrafts}
        dirtyChannels={dirtyChannels}
        name={name}
        url={url}
        groupId={groupId}
        tier={tier}
        vertical={vertical}
        delaySecs={delaySecs}
        excludeFromStats={excludeFromStats}
        liveOnly={liveOnly}
        onNameChange={setName}
        onUrlChange={setUrl}
        onGroupIdChange={setGroupId}
        onTierChange={setTier}
        onVerticalChange={setVertical}
        onDelaySecsChange={setDelaySecs}
        onExcludeFromStatsChange={setExcludeFromStats}
        onLiveOnlyChange={setLiveOnly}
        onInjectSimChannel={injectSimChannel}
        onAddChannel={(e) => void addChannel(e)}
        onUpdateChannelDraft={updateChannelDraft}
        onSaveChannelRow={(id) => void saveChannelRow(id)}
        onToggleChannelEnabled={(ch) => void toggleChannelEnabled(ch)}
        onRemoveChannel={(id) => void removeChannel(id)}
        onChangeTier={(id, next) => void changeTier(id, next)}
        onBulkUpdateChannels={bulkUpdateChannels}
      />
    </div>
  );
}

import { Plus, Trash2 } from "lucide-react";
import { ANALYTICS_GLOSSARY } from "../../utils/analyticsGlossary";
import { LanguageGroupConfig } from "../../types";
import { InfoTip } from "../InfoTip";

interface StreamGroupsCardProps {
  groups: LanguageGroupConfig[];
  groupDrafts: Record<string, LanguageGroupConfig>;
  keywordDrafts: Record<string, string>;
  dirtyGroups: Record<string, boolean>;
  channelCountsByGroup: Record<string, number>;
  newLangCode: string;
  newLangName: string;
  onNewLangCodeChange: (v: string) => void;
  onNewLangNameChange: (v: string) => void;
  onAddGroup: (e: React.FormEvent) => void;
  onChangeGroupDraft: (id: string, next: LanguageGroupConfig) => void;
  onChangeKeywordDraft: (id: string, raw: string) => void;
  onSaveGroup: (id: string) => void;
  onRemoveGroup: (id: string) => void;
}

export function StreamGroupsCard({
  groups,
  groupDrafts,
  keywordDrafts,
  dirtyGroups,
  channelCountsByGroup,
  newLangCode,
  newLangName,
  onNewLangCodeChange,
  onNewLangNameChange,
  onAddGroup,
  onChangeGroupDraft,
  onChangeKeywordDraft,
  onSaveGroup,
  onRemoveGroup,
}: StreamGroupsCardProps) {
  return (
    <section className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5 space-y-4">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <h2 className="text-sm font-medium text-[var(--text-primary)]">
            Stream Groups & Categories
          </h2>
          <p className="text-xs text-[var(--text-muted)]">
            Organize feeds by language, stage, or output, and set live-chat keywords to detect viewer complaints automatically
          </p>
        </div>
        <span className="font-mono-tabular text-xs text-[var(--text-muted)]">
          {groups.length} group{groups.length === 1 ? "" : "s"}
        </span>
      </div>

      <form onSubmit={onAddGroup} className="grid grid-cols-1 gap-3 md:grid-cols-5">
        <div>
          <label htmlFor="lang-code" className="mb-1 block text-xs text-[var(--text-muted)]">
            Group Tag
          </label>
          <input
            id="lang-code"
            type="text"
            required
            maxLength={16}
            value={newLangCode}
            onChange={(e) => onNewLangCodeChange(e.target.value)}
            placeholder="STAGE-A / EN"
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm uppercase font-mono-tabular text-[var(--text-primary)]"
          />
        </div>
        <div className="md:col-span-3">
          <label htmlFor="lang-name" className="mb-1 block text-xs text-[var(--text-muted)]">
            Group Name
          </label>
          <input
            id="lang-name"
            type="text"
            required
            maxLength={80}
            value={newLangName}
            onChange={(e) => onNewLangNameChange(e.target.value)}
            placeholder="Main Stage / Public EN Broadcast"
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm text-[var(--text-primary)]"
          />
        </div>
        <div className="flex items-end">
          <button
            type="submit"
            className="inline-flex h-9 w-full items-center justify-center gap-1.5 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-raised)] px-3 text-sm font-medium text-[var(--text-primary)] hover:bg-[var(--bg-page)]"
          >
            <Plus size={15} strokeWidth={1.5} />
            <span>Add Group</span>
          </button>
        </div>
      </form>

      <div className="divide-y divide-[var(--border-hairline)] overflow-hidden rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)]/40">
        <div className="hidden bg-[var(--bg-page)]/60 px-3 py-2 text-[11px] font-medium text-[var(--text-muted)] md:grid md:grid-cols-[5rem_minmax(150px,1fr)_minmax(240px,1.4fr)_auto] md:items-center md:gap-2.5">
          <span>Group Tag</span>
          <span>Group Name</span>
          <span className="flex items-center gap-1.5">
            <InfoTip entry={ANALYTICS_GLOSSARY.chatComplaintKeywords} underline focusable>
              <span>Chat Comment Problem Keywords</span>
            </InfoTip>
            <span className="text-[10px] font-normal text-[var(--text-muted)]/80">
              (auto-flags live chat complaints)
            </span>
          </span>
          <span className="min-w-[148px] text-right">Feeds & Actions</span>
        </div>
        {groups.map((g) => {
          const draft = groupDrafts[g.id] ?? g;
          const isDirty = Boolean(dirtyGroups[g.id]);
          const feedCount = channelCountsByGroup[g.id] ?? 0;
          const canDelete = groups.length > 1 && feedCount === 0;
          return (
            <div
              key={g.id}
              className="flex flex-col gap-2 px-3 py-2.5 text-xs md:grid md:grid-cols-[5rem_minmax(150px,1fr)_minmax(240px,1.4fr)_auto] md:items-center md:gap-2.5"
            >
              <div>
                <label
                  htmlFor={`group-tag-${g.id}`}
                  className="mb-1 block text-[11px] text-[var(--text-muted)] md:hidden"
                >
                  Group Tag
                </label>
                <input
                  id={`group-tag-${g.id}`}
                  aria-label={`Tag for ${g.name}`}
                  type="text"
                  maxLength={16}
                  value={draft.language_code}
                  onChange={(e) =>
                    onChangeGroupDraft(g.id, { ...draft, language_code: e.target.value })
                  }
                  className="h-7 w-full rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 font-mono-tabular uppercase text-[var(--text-primary)]"
                />
              </div>
              <div>
                <label
                  htmlFor={`group-name-${g.id}`}
                  className="mb-1 block text-[11px] text-[var(--text-muted)] md:hidden"
                >
                  Group Name
                </label>
                <input
                  id={`group-name-${g.id}`}
                  aria-label={`Group name for ${g.language_code}`}
                  type="text"
                  maxLength={80}
                  value={draft.name}
                  onChange={(e) =>
                    onChangeGroupDraft(g.id, { ...draft, name: e.target.value })
                  }
                  className="h-7 w-full rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-[var(--text-primary)]"
                />
              </div>
              <div>
                <label
                  htmlFor={`group-kw-${g.id}`}
                  className="mb-1 block text-[11px] text-[var(--text-muted)] md:hidden"
                >
                  <InfoTip entry={ANALYTICS_GLOSSARY.chatComplaintKeywords} underline focusable>
                    <span>Chat Comment Problem Keywords</span>
                  </InfoTip>
                </label>
                <input
                  id={`group-kw-${g.id}`}
                  aria-label={`Chat comment problem keywords for ${g.language_code}`}
                  type="text"
                  placeholder="e.g. no sound, muted, bisu, lag, freeze"
                  value={keywordDrafts[g.id] ?? draft.complaint_keywords.join(", ")}
                  onChange={(e) => onChangeKeywordDraft(g.id, e.target.value)}
                  className="h-7 w-full rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-primary)]"
                />
              </div>
              <div className="flex min-w-[148px] items-center justify-end gap-2">
                <span className="rounded border border-[var(--border-hairline)] bg-[var(--bg-page)] px-2 py-0.5 font-mono-tabular text-[11px] text-[var(--text-muted)]">
                  {feedCount} feed{feedCount === 1 ? "" : "s"}
                </span>
                <button
                  type="button"
                  disabled={!isDirty}
                  onClick={() => onSaveGroup(g.id)}
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
                  aria-label={`Delete group ${g.language_code}`}
                  disabled={!canDelete}
                  title={
                    groups.length <= 1
                      ? "At least 1 stream group required"
                      : feedCount > 0
                        ? "Reassign feeds before deleting this group"
                        : "Delete stream group"
                  }
                  onClick={() => onRemoveGroup(g.id)}
                  className="inline-flex h-7 w-7 items-center justify-center rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] hover:text-[#e5484d] disabled:opacity-35 disabled:cursor-not-allowed"
                >
                  <Trash2 size={13} strokeWidth={1.5} />
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

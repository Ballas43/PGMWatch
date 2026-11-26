import { useEffect, useRef, useState } from "react";
import { ArrowLeftRight, Download, FilePlus2, Lock, Trash2, Upload } from "lucide-react";
import { ShowPresetSummary } from "../../types";

interface ShowPresetsCardProps {
  activeShowId: string;
  activeSegmentLabel: string | null;
  presets: ShowPresetSummary[];
  loading: boolean;
  onExportPreset: (showId: string) => void;
  onSwitchPreset: (showId: string) => Promise<void>;
  onDeletePreset: (showId: string) => Promise<void>;
  onCreateNewShow: (input: {
    title: string;
    groupCode: string;
    groupName: string;
    resetAccounting: boolean;
  }) => Promise<boolean>;
  onImportPresetJson: (file: File, resetAccounting: boolean) => Promise<boolean>;
}

function formatUpdatedClock(iso: string): string {
  const dt = new Date(iso);
  if (Number.isNaN(dt.getTime())) return "recently";
  return dt.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function ShowPresetsCard({
  activeShowId,
  activeSegmentLabel,
  presets,
  loading,
  onExportPreset,
  onSwitchPreset,
  onDeletePreset,
  onCreateNewShow,
  onImportPresetJson,
}: ShowPresetsCardProps) {
  const [newShowOpen, setNewShowOpen] = useState(false);
  const [newTitle, setNewTitle] = useState("");
  const [starterCode, setStarterCode] = useState("MAIN");
  const [starterName, setStarterName] = useState("Main Broadcast");
  const [resetAccounting, setResetAccounting] = useState(false);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const fileInputRef = useRef<HTMLInputElement>(null);
  const newTitleRef = useRef<HTMLInputElement>(null);
  const deleteResetRef = useRef<number | null>(null);

  const segmentLocked = Boolean(activeSegmentLabel);

  useEffect(() => {
    if (newShowOpen) {
      newTitleRef.current?.focus();
    }
  }, [newShowOpen]);

  useEffect(() => {
    return () => {
      if (deleteResetRef.current !== null) {
        window.clearTimeout(deleteResetRef.current);
      }
    };
  }, []);

  const handleSubmitNewShow = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = newTitle.trim();
    if (!trimmed || busyAction || segmentLocked) return;
    setBusyAction("create");
    try {
      const ok = await onCreateNewShow({
        title: trimmed,
        groupCode: starterCode.trim() || "MAIN",
        groupName: starterName.trim() || "Main Broadcast",
        resetAccounting,
      });
      if (ok) {
        setNewTitle("");
        setStarterCode("MAIN");
        setStarterName("Main Broadcast");
        setResetAccounting(false);
        setNewShowOpen(false);
      }
    } finally {
      setBusyAction(null);
    }
  };

  const handleFileChange = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file || segmentLocked) return;
    setBusyAction("import");
    try {
      await onImportPresetJson(file, false);
    } finally {
      setBusyAction(null);
    }
  };

  const handleSwitch = async (showId: string) => {
    if (segmentLocked || busyAction) return;
    setBusyAction(`switch:${showId}`);
    try {
      await onSwitchPreset(showId);
    } finally {
      setBusyAction(null);
    }
  };

  const handleDeleteClick = async (showId: string) => {
    if (busyAction) return;
    if (confirmDeleteId !== showId) {
      setConfirmDeleteId(showId);
      if (deleteResetRef.current !== null) {
        window.clearTimeout(deleteResetRef.current);
      }
      deleteResetRef.current = window.setTimeout(() => setConfirmDeleteId(null), 4000);
      return;
    }
    setBusyAction(`delete:${showId}`);
    try {
      await onDeletePreset(showId);
      setConfirmDeleteId(null);
    } finally {
      setBusyAction(null);
    }
  };

  return (
    <section
      aria-labelledby="show-presets-heading"
      className="space-y-4 rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5"
    >
      <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div>
          <h2 id="show-presets-heading" className="text-sm font-medium text-[var(--text-primary)]">
            Show Presets & Portability
          </h2>
          <p className="mt-0.5 text-xs text-[var(--text-muted)]">
            Export your show lineup to JSON, switch between saved presets, or start a fresh show replacing all active feeds.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => onExportPreset(activeShowId)}
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-primary)] transition-colors hover:border-[var(--border-strong)]"
          >
            <Download size={14} strokeWidth={1.5} aria-hidden="true" />
            <span>Export Show JSON</span>
          </button>

          <input
            ref={fileInputRef}
            type="file"
            accept=".json,application/json"
            className="hidden"
            onChange={(e) => void handleFileChange(e)}
          />
          <button
            type="button"
            disabled={segmentLocked || busyAction !== null}
            onClick={() => fileInputRef.current?.click()}
            title={
              segmentLocked
                ? `End active segment "${activeSegmentLabel}" before importing a show preset`
                : "Import an exported .pgmwatch-show.json preset and activate it"
            }
            className="inline-flex h-8 items-center gap-1.5 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-primary)] transition-colors hover:border-[var(--border-strong)] disabled:cursor-not-allowed disabled:opacity-45"
          >
            <Upload size={14} strokeWidth={1.5} aria-hidden="true" />
            <span>{busyAction === "import" ? "Importing..." : "Import JSON"}</span>
          </button>

          <button
            type="button"
            disabled={segmentLocked || busyAction !== null}
            aria-expanded={newShowOpen}
            onClick={() => setNewShowOpen((prev) => !prev)}
            title={
              segmentLocked
                ? `End active segment "${activeSegmentLabel}" before overwriting the active show`
                : "Create a brand-new show overwriting all current feeds and groups"
            }
            className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-3 text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${
              newShowOpen
                ? "border-[var(--border-strong)] bg-[var(--bg-page)] text-[var(--text-primary)]"
                : "border-[var(--warning)]/50 bg-[var(--warning-soft)] text-[var(--warning-text)] hover:bg-[var(--warning)]/20"
            }`}
          >
            <FilePlus2 size={14} strokeWidth={1.5} aria-hidden="true" />
            <span>New Show (Overwrite)</span>
          </button>
        </div>
      </div>

      {segmentLocked && (
        <div
          role="status"
          className="flex items-center gap-2 rounded-lg border border-[var(--warning)]/40 bg-[var(--warning-soft)] px-3 py-2 text-xs text-[var(--warning-text)]"
        >
          <Lock size={14} strokeWidth={1.5} className="shrink-0" aria-hidden="true" />
          <span>
            Segment <span className="font-medium">&ldquo;{activeSegmentLabel}&rdquo;</span> is currently recording.
            End the segment in Broadcast Accounting before switching, importing, or overwriting the show. Export remains available.
          </span>
        </div>
      )}

      {newShowOpen && (
        <form
          onSubmit={(e) => void handleSubmitNewShow(e)}
          className="space-y-3 rounded-lg border border-[var(--border-strong)] bg-[var(--bg-page)] p-4"
        >
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-6">
            <div className="sm:col-span-3">
              <label htmlFor="new-show-title" className="mb-1 block text-xs text-[var(--text-muted)]">
                New Show Title
              </label>
              <input
                ref={newTitleRef}
                id="new-show-title"
                type="text"
                required
                maxLength={160}
                placeholder="e.g. Grand Finals Day 2"
                value={newTitle}
                onChange={(e) => setNewTitle(e.target.value)}
                className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-sm text-[var(--text-primary)]"
              />
            </div>

            <div className="sm:col-span-1">
              <label htmlFor="new-show-group-code" className="mb-1 block text-xs text-[var(--text-muted)]">
                Starter Tag
              </label>
              <input
                id="new-show-group-code"
                type="text"
                maxLength={16}
                placeholder="MAIN"
                value={starterCode}
                onChange={(e) => setStarterCode(e.target.value.toUpperCase())}
                className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-sm font-mono-tabular text-[var(--text-primary)]"
              />
            </div>

            <div className="sm:col-span-2">
              <label htmlFor="new-show-group-name" className="mb-1 block text-xs text-[var(--text-muted)]">
                Starter Stream Group
              </label>
              <input
                id="new-show-group-name"
                type="text"
                maxLength={80}
                placeholder="Main Broadcast"
                value={starterName}
                onChange={(e) => setStarterName(e.target.value)}
                className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-sm text-[var(--text-primary)]"
              />
            </div>
          </div>

          <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
            <label className="inline-flex cursor-pointer items-center gap-2 text-xs text-[var(--text-secondary)]">
              <input
                id="new-show-reset-acct"
                type="checkbox"
                checked={resetAccounting}
                onChange={(e) => setResetAccounting(e.target.checked)}
                className="h-4 w-4 rounded border-[var(--border-hairline)] accent-emerald-500"
              />
              <span>Also reset Broadcast Accounting & match history for a fresh start</span>
            </label>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => setNewShowOpen(false)}
                className="h-8 rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-xs font-medium text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
              >
                Cancel
              </button>
              <button
                type="submit"
                disabled={!newTitle.trim() || busyAction === "create" || segmentLocked}
                className="inline-flex h-8 items-center gap-1.5 rounded-lg bg-[var(--text-primary)] px-3.5 text-xs font-medium text-[var(--bg-rail)] disabled:opacity-45"
              >
                <FilePlus2 size={13} strokeWidth={1.5} aria-hidden="true" />
                <span>{busyAction === "create" ? "Creating..." : "Overwrite & Start New Show"}</span>
              </button>
            </div>
          </div>

          <p className="text-[11px] text-[var(--text-muted)]">
            Replaces all active channels and groups on the monitor wall cleanly. Previous shows stay in Saved Show Presets below so you can switch back anytime.
          </p>
        </form>
      )}

      <div>
        <div className="mb-2 flex items-center justify-between">
          <h3 className="text-xs font-medium text-[var(--text-secondary)]">
            Saved Show Presets ({presets.length})
          </h3>
        </div>

        {loading && presets.length === 0 ? (
          <div className="space-y-2" role="status" aria-busy="true" aria-label="Loading saved show presets">
            <div className="h-11 animate-pulse rounded-lg bg-[var(--bg-raised)]" />
            <div className="h-11 animate-pulse rounded-lg bg-[var(--bg-raised)]" />
          </div>
        ) : presets.length === 0 ? (
          <div
            role="status"
            className="rounded-lg border border-dashed border-[var(--border-hairline)] px-4 py-6 text-center text-xs text-[var(--text-muted)]"
          >
            No saved show presets yet. Export or create a new show to populate your preset library.
          </div>
        ) : (
          <ul role="list" className="divide-y divide-[var(--border-hairline)] rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-page)]">
            {presets.map((p) => {
              const isSwitching = busyAction === `switch:${p.show_id}`;
              const isDeleting = busyAction === `delete:${p.show_id}`;
              const confirmingDelete = confirmDeleteId === p.show_id;
              return (
                <li
                  key={p.show_id}
                  className="flex flex-col gap-2 px-3.5 py-2.5 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="truncate text-xs font-medium text-[var(--text-primary)]">
                        {p.title}
                      </span>
                      {p.is_active && (
                        <span className="rounded border border-[var(--positive)]/40 bg-[var(--positive-soft)] px-1.5 py-0.5 text-[10px] font-mono-tabular font-medium text-[var(--positive-text)]">
                          Active
                        </span>
                      )}
                    </div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-2 text-[11px] font-mono-tabular text-[var(--text-muted)]">
                      <span>{p.group_count} {p.group_count === 1 ? "group" : "groups"}</span>
                      <span>·</span>
                      <span>{p.channel_count} {p.channel_count === 1 ? "feed" : "feeds"}</span>
                      <span>·</span>
                      <span>Updated {formatUpdatedClock(p.updated_at)}</span>
                    </div>
                  </div>

                  <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                    {!p.is_active && (
                      <button
                        type="button"
                        disabled={segmentLocked || busyAction !== null}
                        onClick={() => void handleSwitch(p.show_id)}
                        aria-label={`Switch to show preset ${p.title}`}
                        title={
                          segmentLocked
                            ? `End active segment "${activeSegmentLabel}" before switching presets`
                            : `Load "${p.title}" onto the monitoring wall`
                        }
                        className="inline-flex h-7 items-center gap-1.5 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2.5 text-xs font-medium text-[var(--text-primary)] transition-colors hover:border-[var(--positive)]/50 hover:text-[var(--positive-text)] disabled:cursor-not-allowed disabled:opacity-45"
                      >
                        <ArrowLeftRight size={12} strokeWidth={1.5} aria-hidden="true" />
                        <span>{isSwitching ? "Switching..." : "Switch"}</span>
                      </button>
                    )}

                    <button
                      type="button"
                      onClick={() => onExportPreset(p.show_id)}
                      aria-label={`Export ${p.title} JSON preset`}
                      title={`Download ${p.title} as JSON`}
                      className="inline-flex h-7 items-center gap-1 rounded border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-2 text-xs text-[var(--text-secondary)] transition-colors hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
                    >
                      <Download size={12} strokeWidth={1.5} aria-hidden="true" />
                      <span>Export</span>
                    </button>

                    {!p.is_active && (
                      <button
                        type="button"
                        disabled={busyAction !== null}
                        onClick={() => void handleDeleteClick(p.show_id)}
                        aria-label={
                          confirmingDelete
                            ? `Confirm deleting preset ${p.title}`
                            : `Delete preset ${p.title}`
                        }
                        className={`inline-flex h-7 items-center gap-1 rounded border px-2 text-xs transition-colors disabled:opacity-45 ${
                          confirmingDelete
                            ? "border-[var(--negative)]/60 bg-[var(--negative-soft)] font-medium text-[var(--negative-text)]"
                            : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] hover:border-[var(--negative)]/50 hover:text-[var(--negative-text)]"
                        }`}
                      >
                        <Trash2 size={12} strokeWidth={1.5} aria-hidden="true" />
                        <span>{isDeleting ? "Deleting..." : confirmingDelete ? "Confirm" : "Delete"}</span>
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </section>
  );
}

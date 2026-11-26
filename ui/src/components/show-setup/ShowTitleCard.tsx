interface ShowTitleCardProps {
  showTitleDraft: string;
  showTitleValue: string;
  dirtyShowTitle: boolean;
  onChangeTitle: (next: string) => void;
  onSubmit: (e: React.FormEvent) => void;
}

export function ShowTitleCard({
  showTitleDraft,
  showTitleValue,
  dirtyShowTitle,
  onChangeTitle,
  onSubmit,
}: ShowTitleCardProps) {
  const canSave = dirtyShowTitle && showTitleDraft.trim() !== showTitleValue;
  return (
    <section className="rounded-xl border border-[var(--border-hairline)] bg-[var(--bg-card)] p-5">
      <form onSubmit={onSubmit} className="flex flex-col gap-3 sm:flex-row sm:items-end">
        <div className="min-w-0 flex-1">
          <label htmlFor="show-title-input" className="block text-sm font-medium text-[var(--text-primary)]">
            Show / Broadcast Title
          </label>
          <p className="mb-2 text-xs text-[var(--text-muted)]">
            Shown in the top header and included in Excel accounting reports (renaming does not interrupt running feeds)
          </p>
          <input
            id="show-title-input"
            type="text"
            required
            maxLength={160}
            placeholder="e.g. Weekend Live Broadcast"
            value={showTitleDraft}
            onChange={(e) => onChangeTitle(e.target.value)}
            className="h-9 w-full rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] px-3 text-sm text-[var(--text-primary)]"
          />
        </div>
        <button
          type="submit"
          disabled={!canSave}
          className={`h-9 shrink-0 rounded-lg border px-4 text-xs font-medium transition-colors ${
            canSave
              ? "border-[var(--positive)]/60 bg-[var(--positive-soft)] text-[var(--positive-text)] hover:bg-[var(--positive)]/25"
              : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-muted)] opacity-50 cursor-default"
          }`}
        >
          Save Title
        </button>
      </form>
    </section>
  );
}

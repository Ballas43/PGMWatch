import { useEffect, useRef, useState } from "react";
import { Trash2, X } from "lucide-react";

interface ClipModalProps {
  clipFilename: string | null;
  title: string;
  onClose: () => void;
  onDeleteClip?: (filename: string) => Promise<void>;
}

export function ClipModal({ clipFilename, title, onClose, onDeleteClip }: ClipModalProps) {
  const closeBtnRef = useRef<HTMLButtonElement>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [deleting, setDeleting] = useState(false);

  useEffect(() => {
    setConfirmDelete(false);
    setDeleting(false);
    if (!clipFilename) return;
    closeBtnRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clipFilename, onClose]);

  const handleDelete = async () => {
    if (!clipFilename || !onDeleteClip) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      window.setTimeout(() => setConfirmDelete(false), 4000);
      return;
    }
    setDeleting(true);
    try {
      await onDeleteClip(clipFilename);
      onClose();
    } finally {
      setDeleting(false);
    }
  };

  if (!clipFilename) return null;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="clip-modal-title"
    >
      <div className="w-full max-w-2xl rounded-2xl border border-[var(--border-strong)] bg-[var(--bg-card)] p-4">
        <div className="mb-3 flex items-center justify-between gap-4">
          <div className="min-w-0">
            <h2 id="clip-modal-title" className="truncate text-sm font-medium text-[var(--text-primary)]">
              {title}
            </h2>
            <div className="truncate text-xs font-mono-tabular text-[var(--text-muted)]">
              {clipFilename}
            </div>
            <div className="mt-0.5 text-[11px] text-[var(--text-muted)]">
              Video-IDR-aligned evidence window (~1.5s Audio Dead Air / ~2.5s pre-trigger, zero pre-keyframe audio leak)
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {onDeleteClip && (
              <button
                type="button"
                disabled={deleting}
                onClick={() => void handleDelete()}
                className={`inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-xs font-medium transition-colors ${
                  confirmDelete
                    ? "border-[var(--negative)] bg-[var(--negative-soft)] text-[var(--negative-text)]"
                    : "border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:border-[var(--border-strong)] hover:text-[var(--text-primary)]"
                }`}
              >
                <Trash2 size={13} strokeWidth={1.5} />
                <span>{deleting ? "Deleting..." : confirmDelete ? "Confirm Delete" : "Delete Clip"}</span>
              </button>
            )}
            <button
              ref={closeBtnRef}
              type="button"
              onClick={onClose}
              aria-label="Close clip player"
              className="inline-flex h-8 w-8 items-center justify-center rounded-lg border border-[var(--border-hairline)] bg-[var(--bg-raised)] text-[var(--text-secondary)] hover:text-[var(--text-primary)]"
            >
              <X size={16} strokeWidth={1.5} />
            </button>
          </div>
        </div>

        <video
          src={`/clips/${encodeURIComponent(clipFilename)}`}
          controls
          autoPlay
          className="aspect-video w-full rounded-lg border border-[var(--border-hairline)] bg-black"
        />
      </div>
    </div>
  );
}

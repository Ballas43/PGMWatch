import { ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { MetricGlossaryEntry } from "../utils/analyticsGlossary";

interface InfoTipProps {
  entry: MetricGlossaryEntry;
  children: ReactNode;
  className?: string;
  underline?: boolean;
  /** Set true when wrapping text-only labels so keyboard Tab users can focus and read the tooltip. */
  focusable?: boolean;
  /** Temporarily suppresses tooltip display (e.g. while an attached popover/dialog is open). */
  disabled?: boolean;
}

export function InfoTip({
  entry,
  children,
  className = "",
  underline = false,
  focusable = false,
  disabled = false,
}: InfoTipProps) {
  const [open, setOpen] = useState(false);
  const [coords, setCoords] = useState<{ top: number; left: number; above: boolean } | null>(null);
  const triggerRef = useRef<HTMLSpanElement | null>(null);
  const tipRef = useRef<HTMLDivElement | null>(null);
  const tooltipId = useId();

  useEffect(() => {
    if (disabled && open) setOpen(false);
  }, [disabled, open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return;
    const rect = triggerRef.current.getBoundingClientRect();
    const tipWidth = Math.min(280, window.innerWidth - 16);
    const tipHeight = tipRef.current?.offsetHeight ?? 140;
    const left = Math.max(
      8,
      Math.min(window.innerWidth - tipWidth - 8, rect.left + rect.width / 2 - tipWidth / 2)
    );
    const spaceBelow = window.innerHeight - rect.bottom;
    const above = spaceBelow < tipHeight + 16 && rect.top > tipHeight + 16;
    const top = above
      ? Math.max(tipHeight + 8, rect.top - 8)
      : Math.min(window.innerHeight - tipHeight - 8, rect.bottom + 8);
    setCoords({ top, left, above });
  }, [open, entry.title, entry.summary, entry.guide]);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onScroll = () => setOpen(false);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open]);

  return (
    <span
      ref={triggerRef}
      tabIndex={focusable ? 0 : undefined}
      aria-describedby={!disabled && open ? tooltipId : undefined}
      onMouseEnter={() => {
        if (!disabled) setOpen(true);
      }}
      onMouseLeave={() => setOpen(false)}
      onFocus={() => {
        if (!disabled) setOpen(true);
      }}
      onBlur={() => setOpen(false)}
      className={`cursor-help ${
        underline
          ? "underline decoration-[var(--border-strong)] decoration-dotted underline-offset-[3px]"
          : ""
      } ${focusable ? "rounded-sm focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--positive)]" : ""} ${className}`}
    >
      {children}
      {!disabled &&
        open &&
        coords &&
        createPortal(
          <div
            ref={tipRef}
            id={tooltipId}
            role="tooltip"
            className="pointer-events-none fixed z-[9999] w-[min(280px,calc(100vw-16px))] rounded-lg border border-[var(--border-strong)] bg-[var(--bg-popover)] px-3 py-2 text-left text-[11px] leading-relaxed text-[var(--text-primary)] shadow-xl"
            style={{
              left: `${coords.left}px`,
              top: `${coords.top}px`,
              transform: coords.above ? "translateY(-100%)" : undefined,
            }}
          >
            <div className="font-sans font-semibold text-[var(--text-primary)]">{entry.title}</div>
            <p className="mt-0.5 font-sans text-[11px] text-[var(--text-secondary)]">
              {entry.summary}
            </p>
            {entry.guide && (
              <p className="mt-1 border-t border-[var(--border-hairline)] pt-1 font-mono-tabular text-[10px] text-[var(--positive-text)]">
                {entry.guide}
              </p>
            )}
          </div>,
          document.body
        )}
    </span>
  );
}

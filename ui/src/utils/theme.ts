export type ThemeMode = "dark" | "light";

const STORAGE_KEY = "pgmwatch_theme";

export function readStoredTheme(): ThemeMode {
  try {
    const saved = window.localStorage.getItem(STORAGE_KEY);
    if (saved === "light" || saved === "dark") return saved;
  } catch {
    // Ignore storage unavailability (private browsing / sandboxed webview)
  }
  if (typeof window !== "undefined" && window.matchMedia?.("(prefers-color-scheme: light)").matches) {
    return "light";
  }
  return "dark";
}

export function applyTheme(mode: ThemeMode): void {
  const root = document.documentElement;
  root.dataset.theme = mode;
  root.style.colorScheme = mode;
  try {
    window.localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // Ignore write failure
  }
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) {
    meta.setAttribute("content", mode === "light" ? "#f4f5f7" : "#131519");
  }
}

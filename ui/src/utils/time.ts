function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

let cachedShortTzFormatter: Intl.DateTimeFormat | null | undefined;

function getShortTzFormatter(): Intl.DateTimeFormat | null {
  if (cachedShortTzFormatter !== undefined) return cachedShortTzFormatter;
  try {
    cachedShortTzFormatter = new Intl.DateTimeFormat(undefined, {
      timeZoneName: "short",
    });
  } catch {
    cachedShortTzFormatter = null;
  }
  return cachedShortTzFormatter;
}

/**
 * Returns the browser's short timezone abbreviation (e.g., "PST", "WIB", "GMT+8"),
 * falling back to "Local" if unavailable.
 */
export function getShortTimeZoneName(date: Date = new Date()): string {
  const fmt = getShortTzFormatter();
  if (fmt) {
    try {
      const tzPart = fmt.formatToParts(date).find((p) => p.type === "timeZoneName")?.value?.trim();
      if (tzPart) return tzPart;
    } catch {
      // Ignore Intl fallback errors on restricted runtimes
    }
  }
  return "Local";
}

/**
 * Returns a human-readable label combining the IANA zone and short offset/name
 * (e.g., "Asia/Singapore (GMT+8)" or "Local").
 */
export function getDeviceTimeZoneLabel(): string {
  const shortName = getShortTimeZoneName();
  try {
    const iana = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (iana && shortName && !iana.includes(shortName)) {
      return `${iana} (${shortName})`;
    }
    if (iana) return iana;
  } catch {
    // Ignore Intl errors
  }
  return shortName;
}

/**
 * Formats an ISO timestamp in the device's local timezone as tabular `HH:MM` (24-hour),
 * optionally suffixed with the short local timezone label.
 */
export function formatLocalHm(iso: string, includeTz = true): string {
  const dt = new Date(iso);
  if (Number.isNaN(dt.getTime())) return "--:--";
  const hm = `${pad2(dt.getHours())}:${pad2(dt.getMinutes())}`;
  return includeTz ? `${hm} ${getShortTimeZoneName(dt)}` : hm;
}

/**
 * Formats an ISO timestamp in the device's local timezone as tabular `YYYY-MM-DD HH:mm:ss`.
 */
export function formatLocalDateTime(iso: string): string {
  const dt = new Date(iso);
  if (Number.isNaN(dt.getTime())) return iso;
  return `${dt.getFullYear()}-${pad2(dt.getMonth() + 1)}-${pad2(dt.getDate())} ${pad2(
    dt.getHours()
  )}:${pad2(dt.getMinutes())}:${pad2(dt.getSeconds())}`;
}

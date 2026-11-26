export interface BrandLogoProps {
  /** Pixel width and height of the square logo mark. */
  size?: number;
  /**
   * "badge": faithful dark-slate rounded tile matching Logo.jpg (crisp in both Light and Dark themes).
   * "mark": transparent background vector mark whose outer bezel adapts to theme text ink.
   */
  variant?: "badge" | "mark";
  className?: string;
}

/**
 * Vector recreation of PGMWatch's broadcast frame + emerald signal pulse + coral tally logo (Logo.jpg).
 */
export function BrandLogo({ size = 28, variant = "badge", className = "" }: BrandLogoProps) {
  const isBadge = variant === "badge";
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      aria-hidden="true"
      className={`shrink-0 ${className}`}
    >
      {isBadge && (
        <rect
          x="0.5"
          y="0.5"
          width="31"
          height="31"
          rx="7.5"
          fill="#151821"
          stroke="var(--brand-badge-ring)"
          strokeWidth="1"
        />
      )}
      <path
        d="M20.2 7.4H7.4C5.8 7.4 4.6 8.6 4.6 10.2V21.8C4.6 23.4 5.8 24.6 7.4 24.6H24.4C26 24.6 27.2 23.4 27.2 21.8V14.8"
        stroke={isBadge ? "#F3F5F8" : "currentColor"}
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M8.1 16.2H10.5L11.7 13.7L13.3 18.4L15.3 11.1L17.1 21.1L18.8 14.1L20.1 16.2H23.7"
        stroke="var(--brand-pulse)"
        strokeWidth="2.35"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <circle cx="25.6" cy="8.4" r="2.7" fill="var(--brand-tally)" />
    </svg>
  );
}

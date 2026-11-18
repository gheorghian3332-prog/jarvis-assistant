import type { ReactNode } from "react";

/** One stroke weight and one 24-unit grid for every icon; they inherit `currentColor`. */
function Svg({ children, size = 18 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

export type IconName =
  | "general"
  | "voice"
  | "ai"
  | "system"
  | "visual"
  | "privacy"
  | "memory"
  | "tools"
  | "setup"
  | "gear"
  | "close"
  | "send"
  | "stop"
  | "mic"
  | "compact"
  | "full"
  | "tray";

const PATHS: Record<IconName, ReactNode> = {
  general: (
    <>
      <path d="M4 7h9M17 7h3M4 17h3M11 17h9" />
      <circle cx="15" cy="7" r="2" />
      <circle cx="9" cy="17" r="2" />
    </>
  ),
  voice: (
    <>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3" />
    </>
  ),
  ai: (
    <>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <path d="M9 3v3M15 3v3M9 18v3M15 18v3M3 9h3M3 15h3M18 9h3M18 15h3" />
      <path d="M10 12h4" />
    </>
  ),
  system: (
    <>
      <path d="M3 12h4l2-6 4 12 2-6h6" />
    </>
  ),
  visual: (
    <>
      <path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12Z" />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  privacy: (
    <>
      <path d="M12 3 5 6v5.5c0 4.2 2.9 7.6 7 9.5 4.1-1.9 7-5.3 7-9.5V6l-7-3Z" />
      <path d="m9 12 2.2 2.2L15.5 10" />
    </>
  ),
  memory: (
    <>
      <ellipse cx="12" cy="6" rx="7" ry="3" />
      <path d="M5 6v6c0 1.7 3.1 3 7 3s7-1.3 7-3V6M5 12v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" />
    </>
  ),
  tools: (
    <>
      <path d="M14.5 6.5a4 4 0 0 0 4.9 4.9L20.5 12.5 12.5 20.5a2.1 2.1 0 0 1-3-3l8-8Z" />
      <path d="M14.5 6.5 17 4l3 3-2.5 2.5" />
    </>
  ),
  setup: (
    <>
      <path d="M12 3v12M7 10l5 5 5-5M4 20h16" />
    </>
  ),
  gear: (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M12 2.8v2.4M12 18.8v2.4M2.8 12h2.4M18.8 12h2.4M5.5 5.5l1.7 1.7M16.8 16.8l1.7 1.7M5.5 18.5l1.7-1.7M16.8 7.2l1.7-1.7" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6 6 18" />,
  send: <path d="M4 12 20 4l-4 16-4-6.5L4 12Zm8 1.5L20 4" />,
  stop: (
    <>
      <path d="M8.5 3h7L21 8.5v7L15.5 21h-7L3 15.5v-7L8.5 3Z" />
      <path d="M12 8v4.5M12 15.6v.1" />
    </>
  ),
  mic: (
    <>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5.5 11.5a6.5 6.5 0 0 0 13 0M12 18v3" />
    </>
  ),
  compact: <path d="M4 9h5V4M20 9h-5V4M4 15h5v5M20 15h-5v5" />,
  full: <path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5" />,
  tray: (
    <>
      <path d="M4 8h16M6 8v10h12V8" />
      <path d="M9 12.5h6" />
    </>
  ),
};

export function Icon({ name, size }: { name: IconName; size?: number }) {
  return <Svg size={size}>{PATHS[name]}</Svg>;
}

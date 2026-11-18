import type { AssistantState } from "../api";
import { sanitizeLevel } from "./level";
import { QUALITY_SETTINGS, type QualitySetting } from "./quality";
import { isAssistantState } from "./states";

export interface PreviewParams {
  state: AssistantState | null;
  level: number;
  debug: boolean;
  /** Initial quality in a plain browser (screenshots); ignored inside Tauri where the stored setting wins. */
  quality: QualitySetting | null;
  /** `?settings=VOICE` opens the settings overlay on that section (screenshots); `?settings=1` opens it on the first. */
  settings: string | null;
}

/** Reads `?state=SPEAKING&level=0.6&debug=1&quality=high`; used for screenshots in a plain browser. */
export function parsePreviewParams(search: string): PreviewParams {
  const params = new URLSearchParams(search);
  const rawState = params.get("state")?.toUpperCase() ?? null;
  const rawLevel = params.get("level");
  const rawQuality = params.get("quality");
  return {
    quality: QUALITY_SETTINGS.find((item) => item === rawQuality) ?? null,
    state: isAssistantState(rawState) ? rawState : null,
    level: rawLevel === null ? 0 : sanitizeLevel(Number(rawLevel)),
    debug: params.get("debug") === "1",
    settings: params.get("settings")?.toUpperCase() ?? null,
  };
}

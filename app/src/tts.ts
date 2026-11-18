import type { TtsEngineName, TtsVoice, TtsVoices } from "./api";
import type { Language } from "./settings";

export interface VoiceOption {
  id: string;
  installed: boolean;
}

/**
 * Voices to offer for an engine and language. The currently stored voice is always in the list
 * (even if it is not installed), so the selector never silently shows a different voice.
 */
export function voiceOptions(
  report: TtsVoices | null,
  engine: TtsEngineName,
  lang: "ru" | "en",
  current: string,
): VoiceOption[] {
  const options: VoiceOption[] = (report?.voices ?? [])
    .filter((voice: TtsVoice) => voice.engine === engine && voice.lang === lang)
    .map((voice) => ({ id: voice.id, installed: voice.installed }));
  if (current !== "" && !options.some((option) => option.id === current)) {
    options.push({ id: current, installed: false });
  }
  return options;
}

/** Phrase for the "test voice" button, in the interface language. */
export function testPhrase(language: Language, phrases: Record<Language, string>): string {
  return phrases[language];
}

/** The key field is sent as is and cleared afterwards; empty input is not sent. */
export function keyToSave(input: string): string | null {
  const trimmed = input.trim();
  return trimmed.length === 0 ? null : trimmed;
}

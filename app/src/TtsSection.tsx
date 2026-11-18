import { memo, useCallback, useEffect, useState } from "react";
import {
  audioOutputs,
  ELEVEN_KEY,
  isTauri,
  secretClear,
  secretExists,
  secretSet,
  ttsSpeak,
  ttsStop,
  ttsVoices,
  type OutputDevice,
  type TtsVoices,
} from "./api";
import type { MessageKey } from "./i18n";
import { translate } from "./i18n";
import {
  TTS_ENGINES_EN,
  TTS_SPEED_RANGE,
  validElevenVoiceId,
  type AppSettings,
  type SettingKey,
  type TtsEngineEn,
} from "./settings";
import { keyToSave, testPhrase, voiceOptions } from "./tts";

type Translate = (key: MessageKey) => string;
type OnChange = <K extends SettingKey>(key: K, value: AppSettings[K]) => void;

function message(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

/** Section "Jarvis voice": engines, voices, speed, volume, output, the ElevenLabs key and a test button. */
export const TtsSection = memo(function TtsSection({
  t,
  values,
  onChange,
}: {
  t: Translate;
  values: AppSettings;
  onChange: OnChange;
}) {
  const [report, setReport] = useState<TtsVoices | null>(null);
  const [devices, setDevices] = useState<OutputDevice[]>([]);
  const [keySaved, setKeySaved] = useState(false);
  const [keyInput, setKeyInput] = useState("");
  const [elevenText, setElevenText] = useState(values["tts.eleven_voice_id"]);
  const [elevenError, setElevenError] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const storedEleven = values["tts.eleven_voice_id"];

  const refresh = useCallback(() => {
    if (!isTauri()) return;
    ttsVoices()
      .then(setReport)
      .catch(() => undefined);
    secretExists(ELEVEN_KEY)
      .then(setKeySaved)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    if (!isTauri()) return;
    audioOutputs()
      .then(setDevices)
      .catch(() => undefined);
  }, [refresh]);

  useEffect(() => {
    setElevenText(storedEleven);
    setElevenError(false);
  }, [storedEleven]);

  const commitEleven = () => {
    const text = elevenText.trim();
    if (!validElevenVoiceId(text)) {
      setElevenError(true);
      return;
    }
    setElevenError(false);
    if (text !== storedEleven) onChange("tts.eleven_voice_id", text);
  };

  const saveKey = () => {
    const key = keyToSave(keyInput);
    if (key === null) return;
    // The field is emptied at once: the value lives in the credential store from now on.
    setKeyInput("");
    secretSet(ELEVEN_KEY, key)
      .then(() => {
        setProblem(null);
        refresh();
      })
      .catch((error: unknown) => setProblem(message(error)));
  };

  const deleteKey = () => {
    secretClear(ELEVEN_KEY)
      .then(() => {
        setProblem(null);
        refresh();
      })
      .catch((error: unknown) => setProblem(message(error)));
  };

  const speak = () => {
    const language = values["general.language"];
    const phrases = { ru: translate("ru", "ttsTestPhrase"), en: translate("en", "ttsTestPhrase") };
    ttsSpeak(testPhrase(language, phrases))
      .then(() => setProblem(null))
      .catch((error: unknown) => setProblem(`${t("ttsFailed")}: ${message(error)}`));
  };

  const ruVoice = values["tts.voice_ru"];
  const enVoice = values["tts.voice_en"];
  const enEngine = values["tts.engine_en"];
  const ruOptions = voiceOptions(report, "piper", "ru", ruVoice);
  const enOptions = voiceOptions(report, enEngine, "en", enVoice);
  const device = values["tts.output_device"];
  const knownDevice = device === null || devices.some((item) => item.name === device);
  const optionLabel = (option: { id: string; installed: boolean }) =>
    option.installed || report === null ? option.id : `${option.id} (${t("ttsNotInstalled")})`;

  return (
    <section className="tts" aria-label={t("ttsTitle")}>
      <h3>{t("ttsTitle")}</h3>
      <label className="field">
        <span>{t("ttsEngineRu")}</span>
        <select value={values["tts.engine_ru"]} onChange={() => onChange("tts.engine_ru", "piper")}>
          <option value="piper">Piper</option>
        </select>
      </label>
      <label className="field">
        <span>{t("ttsVoiceRu")}</span>
        <select value={ruVoice} onChange={(event) => onChange("tts.voice_ru", event.target.value)}>
          {ruOptions.map((option) => (
            <option key={option.id} value={option.id}>
              {optionLabel(option)}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>{t("ttsEngineEn")}</span>
        <select
          value={enEngine}
          onChange={(event) => onChange("tts.engine_en", event.target.value as TtsEngineEn)}
        >
          {TTS_ENGINES_EN.map((engine) => (
            <option key={engine} value={engine}>
              {engine === "kokoro" ? "Kokoro" : "Piper"}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>{t("ttsVoiceEn")}</span>
        <select value={enVoice} onChange={(event) => onChange("tts.voice_en", event.target.value)}>
          {enOptions.map((option) => (
            <option key={option.id} value={option.id}>
              {optionLabel(option)}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>
          {t("ttsSpeed")} <span className="mono">{values["tts.speed"].toFixed(2)}</span>
        </span>
        <input
          type="range"
          min={TTS_SPEED_RANGE.min}
          max={TTS_SPEED_RANGE.max}
          step={TTS_SPEED_RANGE.step}
          value={values["tts.speed"]}
          onChange={(event) => onChange("tts.speed", Number(event.target.value))}
        />
      </label>
      <label className="field">
        <span>
          {t("ttsVolume")} <span className="mono">{values["tts.volume"].toFixed(2)}</span>
        </span>
        <input
          type="range"
          min={0}
          max={1}
          step={0.05}
          value={values["tts.volume"]}
          onChange={(event) => onChange("tts.volume", Number(event.target.value))}
        />
      </label>
      <label className="field">
        <span>{t("ttsOutput")}</span>
        <select
          value={device ?? ""}
          onChange={(event) => onChange("tts.output_device", event.target.value === "" ? null : event.target.value)}
        >
          <option value="">{t("ttsOutputDefault")}</option>
          {!knownDevice && device !== null && <option value={device}>{device}</option>}
          {devices.map((item) => (
            <option key={item.name} value={item.name}>
              {item.name}
              {item.is_default ? " ★" : ""}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>
          {t("ttsCloud")}
          <small className="muted block">{t("ttsCloudHint")}</small>
        </span>
        <input
          type="checkbox"
          checked={values["tts.cloud_enabled"]}
          onChange={(event) => onChange("tts.cloud_enabled", event.target.checked)}
        />
      </label>
      <label className="field">
        <span>{t("ttsElevenVoice")}</span>
        <input
          type="text"
          value={elevenText}
          maxLength={64}
          aria-invalid={elevenError}
          onChange={(event) => setElevenText(event.target.value)}
          onBlur={commitEleven}
          onKeyDown={(event) => {
            if (event.key === "Enter") commitEleven();
          }}
        />
      </label>
      {elevenError && <p className="error">{t("ttsInvalidVoice")}</p>}
      <label className="field">
        <span>
          {t("ttsKeyLabel")} <small className="muted">({keySaved ? t("ttsKeySaved") : t("ttsKeyMissing")})</small>
          <small className="muted block">{t("ttsKeyHint")}</small>
        </span>
        <input
          type="password"
          value={keyInput}
          autoComplete="off"
          spellCheck={false}
          onChange={(event) => setKeyInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") saveKey();
          }}
        />
      </label>
      <div className="actions">
        <button type="button" disabled={!isTauri() || keyToSave(keyInput) === null} onClick={saveKey}>
          {t("ttsKeySave")}
        </button>
        <button type="button" disabled={!isTauri() || !keySaved} onClick={deleteKey}>
          {t("ttsKeyDelete")}
        </button>
      </div>
      <div className="actions">
        <button type="button" disabled={!isTauri()} onClick={speak}>
          {t("ttsTest")}
        </button>
        <button type="button" disabled={!isTauri()} onClick={() => void ttsStop().catch(() => undefined)}>
          {t("ttsStop")}
        </button>
      </div>
      {problem && <p className="error">{problem}</p>}
      {report && (
        <ul className="engines" aria-label={t("ttsEngines")}>
          {report.engines.map((engine) => (
            <li key={engine.engine} className={engine.available ? "" : "muted"}>
              <span className="mono">{engine.engine}</span>: {engine.detail}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
});

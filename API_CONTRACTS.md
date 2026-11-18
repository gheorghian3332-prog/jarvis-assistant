# API_CONTRACTS.md — v0.2 (Rust-first)

Меняет только Team Lead (новая версия + CHANGELOG). Источник правды — Rust-типы в `crates/jarvis-core/src/protocol.rs`
(serde + `ts-rs` генерирует `app/src/types/protocol.ts`).

## UI ↔ core (Tauri IPC)
### Commands (UI вызывает `invoke`)
| command | args | returns | фаза |
|---|---|---|---|
| `core_info` | — | `{version, uptime_s, features: string[]}` | 1 |
| `settings_get` | `{key}` | `Value \| null` | 1 |
| `settings_set` | `{key, value}` | `()`; ключ из whitelist схемы | 1 |
| `audit_list` | `{limit}` | `AuditEntry[]` | 1 |
| `set_window_mode` | `{mode: "full"\|"compact"\|"tray"}` | `()` | 2 |
| `ptt` | `{pressed}` | `()` | 4 |
| `chat_send` | `{text}` | `()` | 4 |
| `tts_stop` | — | `()` | 5 |
| `confirm_response` | `{action_id, approved}` | `()` | 6 |
| `emergency_stop` | — | `()` | 6 |
| `memory_list` / `memory_delete` | `{kind}` / `{kind, id}` | — | 9 |

### Events (core → UI, `listen`)
| event | payload | фаза |
|---|---|---|
| `assistant://state` | `{state: IDLE\|LISTENING\|PROCESSING\|SPEAKING\|EXECUTING\|WARNING\|ERROR\|OFFLINE}` | 1 |
| `assistant://level` | `{rms: f32}` ≤ 30 Гц, только LISTENING/SPEAKING | 4–5 |
| `assistant://message` | `{text, lang, skill?}` | 4 |
| `voice://transcript` | `{text, final, lang}` | 4 |
| `confirm://request` | `{action_id, action, summary, risk}` | 6 |
| `action://result` | `{action_id, ok, detail}` | 6 |
| `system://metrics` | `Metrics` (поля `Option`, `null` если датчика нет) | 7 |
| `alert://raised` | `{id, kind, severity: info\|warning\|critical, message}` | 7 |
| `vision://capture` | `{active}` | 8 |

Ошибки команд: `{code: VALIDATION\|FORBIDDEN\|NOT_FOUND\|SYSTEM\|TIMEOUT\|UNAVAILABLE, message}`.

`system://metrics` в Phase 2: `{cpu_percent, ram_used_bytes, ram_total_bytes}`; расширяется в Phase 7 (новые поля — `Option`).
Режим `tray` = окно уничтожено (WebView освобождён); события в UI в этом режиме не шлются.

### Whitelist настроек (v0.2)
| key | тип | по умолчанию |
|---|---|---|
| `general.autostart` | bool | false |
| `general.language` | `"ru"`\|`"en"` | `"ru"` |
| `general.start_mode` | `"tray"`\|`"compact"`\|`"full"` | `"compact"` |
| `ui.last_mode` | `"compact"`\|`"full"` | `"compact"` |
| `visual.quality` | `"auto"`\|`"low"`\|`"medium"`\|`"high"` | `"auto"` |
| `visual.fps_limit` | `30`\|`45`\|`60` | `30` |
| `visual.reduced_motion` | bool | false |
| `voice.enabled` | bool (постоянное прослушивание) | false |
| `voice.input_device` | string \| null | null (системный) |
| `voice.sensitivity` | 0.0..1.0 | 0.5 |
| `voice.wake_names` | string[1..5], ≤ 20 символов | `["лёха","лёх","алексей"]` |
| `voice.ptt_key` | string | `"F8"` |

### Голос (v0.3, Phase 4)
Команды: `ptt{pressed}`, `voice_devices` → `[{id, name, is_default}]`, `voice_status` → `{listening, worker_running, mic_ok, device}`.
События: `voice://status` (как `voice_status`), `voice://wake` `{name, confidence}`, `voice://transcript` `{text, final, lang}`.
Шина ядра: `Event::WakeDetected`, `Event::Utterance{pcm16k, source: wake|ptt}`, `Event::VoiceStateHint`, `Event::Transcript{text, lang, source}`.
### Голос Jarvis (v0.4, Phase 5)
Команды: `tts_speak{text}` (прерывает текущую речь), `tts_stop`, `tts_voices`, `audio_outputs`, `secret_set{name,value}` / `secret_clear{name}` /
`secret_exists{name}` (значение никогда не возвращается, хранится в Windows Credential Manager). Настройки: `tts.engine_ru`=`piper`, `tts.voice_ru`=`ru_RU-dmitri-medium`,
`tts.engine_en`=`kokoro|piper`, `tts.voice_en`=`bm_george`, `tts.speed` 0.7..1.4, `tts.volume` 0..1, `tts.output_device`, `tts.cloud_enabled`=false, `tts.eleven_voice_id`.

Wake word: Vosk small-ru в дочернем процессе `jarvis.exe --wake-worker` (≈550 МБ, только во время речи, выход после 60 с тишины).

### Мастер первого запуска (v0.11, P11)
Команды: `setup_status` → `{items:[{id, installed, optional, approx_mb, running}], installed_layout, models_dir, ollama_url, default_model}`,
`setup_install{item}` (`vosk_model|piper|piper_voice|stt_env|whisper|kokoro|kokoro_env`; прогресс — событие `setup://progress`
`{item, state: running|done|error, done, total, message}`), `setup_pull_model{model}` (локальный Ollama, item `ollama_model`),
`setup_open_ollama_page`. Настройка `setup.completed` (bool, по умолчанию false). Командная строка: `jarvis.exe --setup-status <file.json>`.
Переменные окружения: `JARVIS_DATA_DIR`, `JARVIS_MODELS_DIR`, `JARVIS_WORKERS_DIR` (окружения воркеров, по умолчанию `%LOCALAPPDATA%\Jarvis\workers`),
`JARVIS_OLLAMA_URL` (только локальный адрес); тестовые хуки `JARVIS_E2E_TTS_WAV`, `JARVIS_E2E_VOICE_WAV`, `JARVIS_E2E_VOICE_GAP_SECS`.

## core ↔ Python-воркер (stdin/stdout)
Кадр: `u32 LE len` (тип+payload) · `u8 type` · `payload`.
| type | направление | payload |
|---|---|---|
| 0x01 HELLO | worker→core | JSON `{worker, version, device, model?, compute_type?}` — после загрузки модели и прогрева |
| 0x02 PING / 0x03 PONG | core→worker / worker→core | произвольные байты, PONG их повторяет |
| 0x10 AUDIO | core→worker | PCM16 mono 16 кГц, сырые байты |
| 0x11 END_UTTERANCE | core→worker | JSON `{lang_hint?}` |
| 0x20 TRANSCRIPT | worker→core | JSON `{text, lang, final, ms}` |
| 0x30 TTS_REQ / 0x31 TTS_PCM | core↔kokoro | JSON `{text, voice, speed}` / PCM16 24 кГц |
| 0x7E ERROR | worker→core | JSON `{code, message}` |
| 0x7F SHUTDOWN | core→worker | — (воркер завершается ≤ 2 с, иначе kill) |

## SQLite v1 (`%APPDATA%\Jarvis\jarvis.db`)
`schema_version(version)` · `settings(key PK, value JSON, updated_at)` · `audit_log(id, ts, action, args JSON, risk, ok, detail)`.
Фазы 6–9: `apps, command_history, devices, projects, scenarios, permissions, memory_facts`.

## Внутренние трейты (Rust)
```rust
#[async_trait] pub trait Service { fn name(&self) -> &'static str; async fn start(&self) -> Result<()>; async fn stop(&self) -> Result<()>; fn critical(&self) -> bool; }
#[async_trait] pub trait Action { fn name(&self) -> &'static str; fn risk(&self) -> Risk; fn validate(&self, args: &Value) -> Result<()>; async fn run(&self, args: Value) -> Result<ActionResult>; }
pub struct EventBus { /* tokio::sync::broadcast<Arc<Event>>, ёмкость 256, Event — enum, без JSON внутри процесса */ }
```

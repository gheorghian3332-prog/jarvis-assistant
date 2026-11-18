# ARCHITECTURE.md — J.A.R.V.I.S. (Rust-first, v2)

Desktop-ассистент для Windows 11. Локальный, без Claude API. Приоритет — минимум RAM/CPU в фоне.
Решения меняет только Team Lead; каждое — строка в «Журнале решений».

## Референсная машина
6-ядерный десктопный CPU · 16 ГБ RAM · GPU класса GTX 1660, 6 ГБ (CC 7.5) · WebView2 · Windows 11 (приблизительные параметры эталонной машины для бюджетов).

## Процессы
```
jarvis.exe — ОДИН постоянный процесс (Rust, Tauri 2, tokio)
 ├─ core (crate jarvis-core): EventBus · Settings/Memory (SQLite) · Action Registry · Audit · Supervisor
 ├─ sysintel: sysinfo + NVML, адаптивный опрос, алерты          ← единственный источник метрик
 ├─ control: apps, windows, audio (WASAPI/COM), processes, power, network, files
 ├─ voice-front: cpal-захват → silero-VAD → wake word (ONNX через `ort`), всё в Rust, CPU
 ├─ tts: ElevenLabs (reqwest stream) / Piper (piper.exe по требованию) → rodio + огибающая
 ├─ brain: FAST PATH (Rust) → Ollama HTTP → облако (опц.)
 ├─ vision: захват (Windows Graphics Capture / GDI) + Windows.Media.Ocr через crate `windows`
 └─ tray, notifications, autostart, global shortcuts (Tauri plugins)
WebView2 окно (React + TS + R3F) — создаётся при показе, УНИЧТОЖАЕТСЯ в режиме трея (освобождает ~100+ МБ)

По требованию (spawn → работа → выход по idle-таймеру):
 ├─ stt-worker (Python, faster-whisper CUDA) — после wake word, держится тёплым N сек (по умолч. 120), потом exit
 ├─ kokoro-worker (Python) — только если выбран Kokoro (EN)
 └─ Ollama (свой сервер) — модель выгружается `keep_alive` по таймеру
```

## Rust vs Python по модулям
| Модуль | Rust | Python | Решение |
|---|---|---|---|
| Core runtime, EventBus | tokio, `broadcast`/`mpsc`, ~2–5 МБ | интерпретатор ≥ 25–40 МБ постоянно | **Rust** |
| Мониторинг | `sysinfo` (переиспользуемый `System`), `nvml-wrapper` | psutil, pynvml | **Rust** (один процесс, без дублирования) |
| Процессы, запуск приложений, окна | `windows` crate: ShellExecuteW, EnumWindows, OpenProcess | pywin32 | **Rust** |
| Громкость/устройства | `windows` COM: IAudioEndpointVolume, IMMDeviceEnumerator | pycaw | **Rust** |
| Уведомления, трей, автозапуск | Tauri plugins | — | **Rust** |
| SQLite память | `rusqlite` (bundled) | sqlite3 | **Rust** |
| Захват звука, VAD, wake word «Лёха» | `cpal` + `ort` (ONNX: silero-vad, openWakeWord mel/embedding/classifier) | openwakeword | **Rust** — постоянная работа, Python тут дорог |
| STT | `whisper-rs` (whisper.cpp; CUDA-сборка требует CUDA Toolkit) | faster-whisper (CTranslate2, int8 на CUDA, готовые колёса) | **Python on-demand**; whisper-rs — бенчмарк в Phase 4 |
| TTS облако | reqwest + стриминг в rodio | — | **Rust** |
| TTS Piper (RU) | готовый `piper.exe`, вызов по требованию | piper-tts | **внешний бинарник**, без Python |
| TTS Kokoro (EN, британский) | ort + свой G2P (espeak-ng) — дорого в разработке | kokoro-onnx | **Python on-demand** |
| LLM | HTTP к Ollama (reqwest) | ollama-python | **Rust** (это просто HTTP) |
| OCR | Windows.Media.Ocr через WinRT | winrt-python | **Rust** |
| VLM | HTTP к Ollama (`qwen2.5vl:3b`/`moondream`) | — | **Rust**-клиент |
| Обучение модели «Джарвис» | — | openWakeWord training (офлайн, разово) | **Python, только dev-время** |

Итог: в фоне работает только Rust. Python — только stt-worker и kokoro-worker, по требованию.

## IPC
- UI ↔ core: Tauri commands (запрос/ответ) + Tauri events (push). В одном процессе, без WebSocket и без HTTP-сервера.
- core ↔ Python-воркер: stdin/stdout, кадры `[u32 LE длина][u8 тип][payload]`; аудио — сырые PCM16 16 кГц, ответы — JSON. Без сокетов, без сериализации аудио в JSON.
- core ↔ Ollama: HTTP localhost (внешний сервис).
- Push метрик в UI — только пока окно существует и видимо, ≤ 1 Гц; в трее подписчиков нет → сбор урезан до алертов.

## Критичное vs необязательное
Критичное (всегда): EventBus, Settings, Action Registry, Audit, tray, emergency stop, alerts (низкая частота).
Необязательное (лениво, выгружается): WebView/UI, voice-front (если выключено постоянное прослушивание), NVML, stt/kokoro воркеры, Ollama-модель, vision.
Каждый необязательный компонент реализует `Service { start, stop, idle_timeout }`; Supervisor останавливает простаивающие.

## Частоты опроса (адаптивно)
| Режим | CPU/RAM | GPU (NVML) | Процессы | Сеть/RTT |
|---|---|---|---|---|
| Трей, нет подписчиков | 0.2 Гц (только для алертов) | 0.1 Гц | 0.05 Гц | 0.1 Гц / RTT 1/60 Гц |
| Окно открыто | 1 Гц | 1 Гц | 0.5 Гц | 1 Гц / 0.1 Гц |

## Профилирование (обязательно, не на глаз)
- `scripts/profile.ps1 -Exe <path> -Seconds 300`: раз в 1 с снимает Working Set, Private Bytes, CPU% (по TotalProcessorTime), потоки, хэндлы → CSV + сводка min/avg/p95/max.
- Встроенная фича `--features profiling`: `tracing` спаны + периодический лог собственной памяти (GetProcessMemoryInfo).
- Каждая фаза фиксирует цифры в docs/PERFORMANCE.md. Регрессия > 20% — блокер.

## Целевые бюджеты (проверяются профилированием)
| Состояние | Private Bytes | CPU (среднее) |
|---|---|---|
| Phase 1 headless core (`jarvisd`) | < 15 МБ | < 0.2% |
| Трей, без прослушивания | < 30 МБ | < 0.3% |
| Трей + постоянный wake word | < 60 МБ | < 2% |
| Окно открыто, Neural Core idle 30 fps | + WebView2 (измерить) | GPU < 10% |

## Безопасность
Без админа · нет сетевых портов наружу (UI в процессе; Ollama/ElevenLabs — исходящие) · секреты в Windows Credential Manager (`keyring` crate) ·
Action Registry с `risk = safe|confirm|forbidden`, LLM вызывает только зарегистрированные действия с валидацией аргументов ·
защищённые процессы (`csrss, wininit, winlogon, lsass, services, smss, svchost, explorer, MsMpEng`) — forbidden ·
удаление — только в корзину и с подтверждением · emergency stop (хоткей) · аудит в SQLite · индикаторы микрофона/захвата экрана.

## Журнал решений
| Дата | Решение | Причина |
|---|---|---|
| 2026-11-18 | Rust-first: Tauri-процесс = core; Python только STT/Kokoro по требованию | доп. требование пользователя: минимум RAM/CPU |
| 2026-11-18 | UI↔core через Tauri IPC, без WS/HTTP-сервера | меньше IPC overhead, нет открытого порта |
| 2026-11-18 | WebView уничтожается в режиме трея | WebView2 — самый тяжёлый компонент в простое |
| 2026-11-18 | Wake word/VAD в Rust через `ort` | постоянная работа; Python в фоне запрещён |
| 2026-11-18 | Ключевое слово по умолчанию — **«Лёха»** (вариации «Лёх», «Алексей»); список имён настраивается, «Джарвис» можно добавить вторым | пожелание пользователя |
| 2026-11-18 | Модель «Лёха» обучается офлайн (openWakeWord: синтетика Piper RU + записи голоса пользователя), инференс в Rust. До готовности модели — fallback: VAD-сегмент ≤ 1.5 с → whisper-tiny ищет «лёха/лёх» | готовой модели под русское имя нет |
| 2026-11-18 | Piper как внешний exe, Kokoro (EN) — Python-воркер | у Kokoro нет RU; G2P на Rust дорог |
| 2026-11-18 | OCR — Windows.Media.Ocr из Rust | встроен в Win11 |
| 2026-11-18 | LLM 3–4B Q4 по умолчанию | 6 ГБ VRAM делится со STT |
| 2026-11-18 | Температура CPU = `null` без LibreHardwareMonitor | LHM требует админ-драйвер |
| 2026-11-18 | Прототип `the earlier prototype script` — источник команд для FAST PATH | не переписывать рабочее |

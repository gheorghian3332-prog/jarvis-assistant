# J.A.R.V.I.S. — заметки для разработчиков (Developer notes)

## Build and profile

Run all Rust checks with `./scripts/check.ps1`. Build the headless daemon with `cargo build --release -p jarvisd`. Profile it with `./scripts/profile.ps1 -Exe target/release/jarvisd.exe -Seconds 300`.

The profiler writes one CSV row per second and prints min/average/p95/max for working set, private bytes, CPU, threads, and handles. It stops the started daemon at the end of the measurement.

## Сборка и профилирование

- Проверки: `powershell -ExecutionPolicy Bypass -File scripts/check.ps1` (fmt, clippy `-D warnings`, test).
- Релизная сборка: `cargo build --release -p jarvisd`. Данные и логи: `%APPDATA%\Jarvis` (переопределяется `JARVIS_DATA_DIR`).
- Профиль: `scripts/profile.ps1 -Exe target/release/jarvisd.exe -Seconds 300 [-Out profile.csv]` — раз в секунду пишет WorkingSet, PrivateBytes, CPU%, Threads, Handles в CSV, печатает min/avg/p95/max и останавливает процесс (принудительно, поэтому `core.stop` в audit_log не пишется).
- Флаг `jarvisd --selfstat-interval <сек>` пишет собственную память и CPU в лог.

## Desktop-приложение

`jarvis.exe` (crate `jarvis-app`, `app/src-tauri`) — постоянно работающее Tauri 2 приложение: держит `jarvis-core` в своём процессе, живёт в трее, окно создаётся по требованию.

- Сборка: `cd app; npm ci; npx tauri build --no-bundle` → `target/release/jarvis.exe`. Режим разработки: `npx tauri dev`.
- Проверки: `scripts/check.ps1` (сначала `typecheck`/`lint`/`test`/`build` фронтенда, затем cargo fmt/clippy/test по workspace; фронтенд собирается первым, потому что crate встраивает `app/dist`).
- Режимы окна: `full` (1280×800), `compact` (420×640, без рамки, поверх всех, в правом нижнем углу рабочей области), `tray` — окно **уничтожается** вместе с WebView2. Крестик окна = трей; выход только через «Выход» в меню трея.
- Трей: левый клик — открыть в `general.start_mode` (compact, если там `tray`) или убрать окно в трей. Хоткей `Ctrl+Alt+J` — переключение трей/последний режим. Второй запуск фокусирует окно первого экземпляра.
- Автозапуск: настройка `general.autostart` (Settings → GENERAL) пишет запись в `HKCU\...\Run` с аргументом `--tray` (запуск без окна).
- Данные и логи: `%APPDATA%\Jarvis` (`JARVIS_DATA_DIR` переопределяет), лог `logs\jarvis.log`.
- Профиль трея: `scripts/profile.ps1 -Exe target/release/jarvis.exe -Args '--tray' -Seconds 60 -IncludeChildren` — суммирует процесс и всех потомков (`msedgewebview2.exe`), колонка `ProcCount`.
- Иконки генерирует `scripts/gen-icons.ps1` (System.Drawing, без зависимостей).

## Neural Core

Главный визуальный элемент окна — 3D-«мозг» из светящихся нейронов (`app/src/core`, three + `@react-three/fiber`, без postprocessing и drei).

- Три draw call: `Points` (нейроны), `LineSegments` (связи, k = 3 ближайших соседа через сетку), `Points` (импульсы бегут по рёбрам, позиция считается в вершинном шейдере). Геометрия детерминированная (seed), строится один раз на уровень качества. В кадре JS меняет только uniforms и вращение группы.
- Качество: `low` 300 / `medium` 1500 / `high` 4000 нейронов; DPR 0.75 / 1.0 / min(devicePixelRatio, 1.5). `visual.quality = auto` — адаптивный контроллер по времени кадра (вниз при > 25 мс дольше 3 с, вверх при быстрых кадрах дольше 10 с, не чаще раза в 5 с); в компактном окне не выше `medium`.
- FPS: `frameloop="demand"` + собственный `requestAnimationFrame`-цикл, который зовёт `invalidate()` не чаще `visual.fps_limit` (IDLE/OFFLINE — не больше 30). При `document.hidden` цикл полностью останавливается.
- Состояния `assistant://state` → цвет и поведение с плавной интерполяцией (~400 мс); уровень голоса из `assistant://level` сглаживается (attack 30 мс, release 200 мс). Пока ядро не шлёт `assistant://level`, уровень равен 0.
- Настройки: Settings → VISUAL (`visual.quality`, `visual.fps_limit`, `visual.reduced_motion`), отладочный оверлей, предпросмотр состояний и уровня голоса (только для предпросмотра).
- Проверка в браузере без Tauri: `cd app; npm run dev` → `http://localhost:5173/` (состояние OFFLINE, метрик нет). Параметры для скриншотов: `?state=SPEAKING&level=0.6&debug=1&quality=high` (`quality` действует только вне Tauri).
- GPU-профиль: `scripts/profile.ps1 -Exe target/release/jarvis.exe -Seconds 60 -IncludeChildren -Gpu` добавляет колонку `GPU3D%` (счётчик `\GPU Engine(*engtype_3D)\Utilization Percentage`, сумма по процессам дерева; без счётчика колонка пустая). Окно должно быть открыто (запуск без `--tray`).

## Голос

Постоянное прослушивание — только Rust (crate `jarvis-voice`), без Python и ONNX Runtime:
`cpal (микрофон, родная частота) → моно f32 → nnnoiseless (RNNoise, 48 кГц) → rubato 16 кГц → webrtc-vad → сегментатор (pre-roll 300 мс, hangover 400 мс) → Vosk small-ru (грамматика из имён + [unk])`.
Vosk работает **только во время речи**. Модель small-ru занимает около 550 МБ private (Windows не возвращает эту память после освобождения внутри процесса), поэтому она живёт в **дочернем процессе** `jarvis.exe --wake-worker <папка модели>` (протокол кадров по stdin/stdout, `wake_worker.rs`/`child.rs`): процесс стартует при первой речи (загрузка около 0.6 с идёт параллельно самой фразе), остаётся пока речь продолжается и закрывается после минуты тишины или смены имён или выключения прослушивания — память возвращается системе. Включено прослушивание — открыт микрофон (RNNoise, VAD, сегментатор в основном процессе, единицы МБ); выключено — микрофон закрыт и worker не запущен. Пока кто-то говорит рядом с включённым прослушиванием, дочерний процесс держит ~550 МБ private (в тихой комнате его нет).

- **Первый запуск на машине:** `powershell -ExecutionPolicy Bypass -File scripts/fetch-models.ps1` — скачивает в `%LOCALAPPDATA%\Jarvis\models` (`JARVIS_MODELS_DIR` переопределяет) `libvosk` (vosk-api 0.3.45 win64) и модель `vosk-model-small-ru-0.22` с проверкой SHA256. `-Piper` добавляет Piper и голос `ru_RU-dmitri-medium` (нужны только для тестовых WAV). Файлы в git не попадают.
- **Сборка требует `libvosk`:** `crates/jarvis-voice/build.rs` берёт `libvosk.lib` из `%LOCALAPPDATA%\Jarvis\models\libvosk` (или `JARVIS_VOSK_LIB_DIR`) и копирует `*.dll` рядом с собранными exe (`target\debug`, `target\release`). Без `fetch-models.ps1` сборка останавливается с понятным сообщением. `scripts/check.ps1` проверяет наличие библиотеки и модели и сам добавляет папку `libvosk` в `PATH` (тестовые exe лежат в служебных папках cargo; при ручном `cargo test` добавьте эту папку в `PATH`).
- **Настройки** (Settings → VOICE): `voice.enabled` (по умолчанию выключено — приватность), `voice.input_device`, `voice.sensitivity` 0..1, `voice.wake_names` (1–5 имён, буквы, ≤ 20 символов), `voice.ptt_key` (по умолчанию `F8`).
- **Wake word «Лёха»:** имя должно быть первым словом фразы (первое распознанное слово, начавшееся не позже 0.6 с от начала речи) либо речь ≤ 1.5 с; порог уверенности Vosk зависит от чувствительности; после срабатывания 1.5 с кулдаун. После имени состояние `LISTENING`: фраза копится до тишины 800 мс (максимум 12 с); если речи нет 5 с — `IDLE`. Готовая фраза уходит событием `Event::Utterance { pcm16k, source: wake|ptt }` в шину (его заберёт stt-worker), состояние `PROCESSING` до `VoiceService::finish_processing()` (или 30 с).
- **Push-to-talk:** глобальный хоткей (`F8`): нажатие = сразу `LISTENING` без wake word, отпускание = конец фразы (не дольше 30 с). При выключенном прослушивании микрофон открывается только на время фразы и без модели. Кнопка «Удерживайте и говорите» во вкладке VOICE делает то же.
- **Анти-самоактивация:** пока `VoiceService::set_speaking(true)` (Phase 5 выставит на время TTS), wake word не анализируется.
- **События в UI:** `assistant://state`, `assistant://level` (`{rms}`, ≤ 30 Гц, только в `LISTENING` и только пока окно существует; RMS с усилением ×4 и обрезкой до 1), `voice://status` (`{active, device, peak, no_signal, error}`), `voice://wake` (`{name}`). Аудио в UI, на диск и в сеть не уходит.
- **Проверка микрофона:** в лог (`jarvis.log`) пишутся имя устройства и пиковый уровень первых 3 с захвата; если пик < 0.0002 (устройство даёт нули), UI показывает «Микрофон не даёт сигнала».
- **Тестовые WAV и прогон:** `scripts/make-test-wavs.ps1` (Piper, 16 кГц моно) создаёт `tests/audio/*.wav` (см. `tests/audio/README.md`). `jarvisd --voice-wav tests\audio\wake_open_notepad.wav [--no-wake]` прогоняет файл через весь пайплайн и печатает события (без расшифровки). Интеграционные тесты: `cargo test -p jarvis-voice --test wav`.
- **Фраза → текст:** `RouterService` (`crates/jarvis-voice/src/router.rs`) слушает шину: `WakeDetected` — в фоне стартует STT-воркер (`SttService::prewarm`, холодный старт ~3 с перекрывается с командой); `Utterance` — `SttService::transcribe(pcm, None)` → `Event::Transcript {text, lang, source}` и `voice://transcript {text, final: true, lang}` в окно, затем `finish_processing()` (IDLE). Ошибка — состояние `ERROR` на 2 с, потом IDLE; пустой текст — сразу IDLE. Воркер ищется через `JARVIS_REPO_ROOT` или вверх от exe до `workers/stt/pyproject.toml`; не найден — лог `UNAVAILABLE`, без паники. `jarvisd --voice-wav f.wav --stt [--no-prewarm] [--realtime]` печатает `Transcript lang=.. text=..` и задержки. Реальный прогон: `cargo test -p jarvis-voice --test e2e_stt -- --ignored --nocapture`.

## Воркеры

Python-воркеры (`workers/<имя>`, uv-проекты) запускаются ядром только по требованию и завершаются после простоя; постоянный фоновый Python запрещён. Общение — бинарные кадры по stdin/stdout (`u32 LE len | u8 type | payload`, `len` включает байт типа), stderr — логи воркера. Сетевых портов нет.

- **STT** (`workers/stt`, faster-whisper / CTranslate2): модель `small`, на GPU `int8_float16`, на CPU `int8`; язык — лучший из ru/en (или `lang_hint`). CUDA берётся из колёс `nvidia-cublas-cu12==12.8.4.1` и `nvidia-cudnn-cu12==9.8.0.87` (CTranslate2 4.9 собран под CUDA 12.8 / cuDNN 9); если CUDA не поднялась, воркер сам переходит на CPU и сообщает `device` в HELLO. Модель кэшируется в `%LOCALAPPDATA%\Jarvis\models\whisper` (первый запуск скачивает ~460 МБ с Hugging Face, дальше работает офлайн; `JARVIS_MODELS_DIR` переопределяет путь, `JARVIS_STT_DEVICE=auto|cuda|cpu`, `JARVIS_STT_MODEL`).
- **Менеджер** (`crates/jarvis-core/src/worker.rs`): `WorkerHandle` (процесс без окна консоли, внутри Job Object — убивает и `uv`, и его дочерний python) и `SttService::transcribe(&[i16], lang_hint)`. Процесс стартует лениво при первом запросе (HELLO: 60 с на первую загрузку, потом 20 с), один запрос за раз, после 120 с простоя (`SttConfig::idle_timeout`) — SHUTDOWN, 2 с ожидания, затем kill. Падение воркера даёт ошибку запроса, следующий запрос запускает новый процесс (без цикла перезапусков). Кадры PING 0x02 / PONG 0x03 — проверка живости (`SttService::ping`).
- Dev-запуск: `WorkerCommand::dev_stt(repo_root)` = `uv run --project workers/stt python -m jarvis_stt`; для релиза интерпретатор и аргументы задаются через `WorkerCommand`.
- Проверки воркера: `cd workers/stt; uv run ruff check . ; uv run ruff format --check . ; uv run mypy . ; uv run pytest` (входит в `scripts/check.ps1`). Медленные тесты с реальной моделью: `uv run pytest -m slow -s` и `cargo test -p jarvis-core --test worker_stt_real -- --ignored --nocapture --test-threads=1`.
- Тестовые WAV (`workers/stt/tests/audio`) синтезированы офлайн через Windows SAPI: `powershell -File workers/stt/tests/make_audio.ps1`.

## Управление Windows
Crate `crates/jarvis-control` и CLI `jarvisctl` — выполнение команд Windows без LLM (только WinAPI, без `SendInput`, `shutdown.exe` и PowerShell).
Интеграция в приложение — задача P6-B.

- **Архитектура:** `Registry` (имя → `Action`, риск `Safe/Confirm/Forbidden`, включение/выключение) → `Executor::run(Intent)` → `Done | NeedsConfirm | Denied | Failed`.
  Confirm-действия ждут `confirm(action_id, approved)` 30 с; `emergency_stop()` сбрасывает ожидающие подтверждения и запланированное выключение. Аудит — через трейт `AuditSink`.
- **Контроллеры:** приложения (Start Menu `.lnk`/`.url`, Steam `libraryfolders.vdf`+`appmanifest_*.acf`, UWP через `shell:AppsFolder`, алиасы), окна (`EnumWindows`, `WM_CLOSE`),
  звук (`IAudioEndpointVolume`, медиа-клавиши через `WM_APPCOMMAND`), файлы (известные папки, песочница путей, удаление только в корзину), процессы, сеть, питание, время.
- **FAST PATH:** `parse(text, lang) -> Option<Intent>` — правила ru/en; неизвестное возвращает `None` (уйдёт в LLM).

```powershell
cargo run -p jarvis-control --bin jarvisctl -- "открой Discord" --dry-run
cargo run -p jarvis-control --bin jarvisctl -- "уменьши громкость до 30 процентов"
cargo run -p jarvis-control --bin jarvisctl -- "выключи компьютер" --yes   # Confirm-действия только с --yes
cargo run -p jarvis-control --bin jarvisctl -- --list-actions
cargo run -p jarvis-control --bin jarvisctl -- --apps dungeon quest
cargo run -p jarvis-control --bin jarvisctl -- --windows
cargo run -p jarvis-control --bin jarvisctl -- --processes
```

Реальные ночные проверки (без звука, блокировок и выключений): `cargo test -p jarvis-control --test real -- --ignored --nocapture --test-threads=1`.

## Установщик, мастер первого запуска, e2e, профили (P10–P11)

- **Установщик (NSIS, per-user, без админа):** `scripts\build-installer.ps1` → `target\release\bundle\nsis\JARVIS-Assistant_0.2.0_x64-setup.exe`. Внутри: `jarvis.exe`
  (в том же exe режимы `--wake-worker`, `--gpu-worker`), `libvosk.dll`, `uv.exe` (официальный релиз, SHA256 закреплён), исходники и `uv.lock` воркеров. Моделей нет:
  при первом запуске вкладка **SETUP** скачивает их (SHA256), создаёт окружения (`uv sync --frozen` в `%LOCALAPPDATA%\Jarvis\workers\<имя>`), проверяет Ollama (без автоустановки)
  и микрофон. Деинсталлятор спрашивает «удалить данные и модели?» (по умолчанию нет; тихий режим данные не удаляет).
- **Проверка установщика:** `scripts\test-installer.ps1` (установка в папку внутри `target`, запуск `--tray`, `--setup-status`, `uv sync`, тихое удаление).
- **Release-сборка для проверок:** `cargo build --release -p jarvis-app --features custom-protocol` (без `custom-protocol` exe ищет dev-сервер).
- **E2E:** `scripts\e2e.ps1` (группы `headless`, `window`, `llm`, `ollama`; окна и GPU-шаги пропускаются, если на переднем плане игра или полноэкранное приложение).
- **Профили:** `scripts\perf.ps1`; результаты и сравнение с бюджетами — `docs\PERFORMANCE.md`. Безопасность — `docs\SECURITY.md`.

### Troubleshooting

- **Нет голоса / «модели не найдены» у запущенного вручную `jarvis.exe`.** Процессы, запущенные из упакованных (MSIX) приложений, видят `%LOCALAPPDATA%` и `%APPDATA%`
  виртуализированными (`%LOCALAPPDATA%\Packages\<package>\LocalCache\...`), а микрофон у них отдаёт нули. Модели, скачанные из такой сессии, обычный `jarvis.exe`
  (ярлык, Проводник) не увидит. Скачивайте модели мастером **изнутри установленного приложения** (вкладка SETUP) или положите их в папку вне профиля
  (например `D:\JarvisModels`) и задайте переменную пользователя `JARVIS_MODELS_DIR`. Тесты и проверки установщика используют `JARVIS_MODELS_DIR` / `JARVIS_DATA_DIR`
  вне `%LOCALAPPDATA%`; проверку уровня микрофона делает вкладка SETUP внутри приложения (индикатор и «нет сигнала»).
- **Микрофон: «buffer underrun or overrun».** Раньше любой xrun драйвера останавливал прослушивание; теперь xrun игнорируется, а упавший захват перезапускается через 10 с.

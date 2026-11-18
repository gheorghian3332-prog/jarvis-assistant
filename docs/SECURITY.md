# Аудит безопасности J.A.R.V.I.S. (P10)

Дата: 2026-11-18. Объём: все Tauri-команды, CSP и capabilities, сетевая поверхность, секреты и логи, права процесса,
файловые пути, инъекции через LLM, установщик и мастер первого запуска. Проверено по коду и на release-сборке
(`scripts/e2e.ps1`, `Get-NetTCPConnection` по дереву процессов).

## 1. Tauri-команды (UI → ядро)

Окно — единственный клиент. Каждая команда проверяет аргументы на стороне Rust; UI-валидация только для удобства.

| Команда | Вход | Проверка |
|---|---|---|
| `core_info`, `voice_status`, `voice_devices`, `audio_outputs`, `tts_voices`, `control_actions`, `ai_status`, `system_snapshot`, `system_alerts`, `setup_status` | — | только чтение |
| `settings_get` / `settings_set` | key, value | key только из белого списка (`settings::WHITELIST`, 43 ключа); значение проверяется по типу и диапазону (`settings::validate`); запись в аудит |
| `audit_list` | limit | `clamp(1, 200)` |
| `set_window_mode` | enum | десериализация в `WindowMode` (full/compact/tray) |
| `ptt` | bool | — |
| `tts_speak` | text | длина ≤ `MAX_SPEAK_CHARS` |
| `tts_stop`, `emergency_stop` | — | — |
| `secret_set` / `secret_clear` / `secret_exists` | name, value | имя только из `SECRET_NAMES`; значение не возвращается, не логируется и не попадает в аудит (в аудит — только имя) |
| `chat_send` | text | непустой, ≤ `MAX_CHAT_CHARS`; дальше FAST PATH → `Executor::run` (единственный путь исполнения) |
| `confirm_response` | action_id, approved | идентификатор должен быть среди ожидающих подтверждений (30 с), иначе NOT_FOUND |
| `ai_unload` | — | — |
| `memory_facts` / `memory_history` | query, limit | **исправлено в P10:** длина запроса ≤ 200 символов; limit ограничен `MAX_LIST` |
| `memory_fact_delete`, `memory_history_delete` | id | числовой id |
| `memory_clear_history` | — | аудит; `memory_clear_all` требует `confirmed = true` |
| `system_top_processes` | by | только `cpu` / `ram` |
| `debug_raise_alert` | severity, speak | в release возвращает FORBIDDEN (`cfg(debug_assertions)`) |
| `setup_install` | item | **новое (P11):** item только из таблицы `setup::ITEMS`; один запуск на элемент одновременно |
| `setup_pull_model` | model | **новое:** имя `[A-Za-z0-9][A-Za-z0-9._:/-]{0,63}`; запрос только на локальный Ollama |
| `setup_open_ollama_page` | — | открывает константный URL https://ollama.com/download, без пользовательского ввода |

Командной строки, произвольных путей и произвольных URL ни одна команда не принимает.

## 2. CSP и capabilities

* CSP: `default-src 'self'; connect-src ipc: http://ipc.localhost; img-src 'self' data:; object-src 'none'; base-uri 'none';
  form-action 'none'; frame-src 'none'` (добавлены `object-src/base-uri/form-action/frame-src` в P10). Внешних скриптов, шрифтов и
  сетевых подключений из WebView нет.
* `withGlobalTauri: false`. Capability `default` — только окно `main`, права `core:event:default` и
  `core:window:allow-start-dragging`. Плагины (autostart, global-shortcut, notification, single-instance) не открывают команд
  фронтенду: их разрешений в capability нет.
* `devUrl` (http://localhost:5173) используется только в dev-сборке; release собирается с `custom-protocol` и грузит встроенные ресурсы.

## 3. Сетевая поверхность

* Приложение **не слушает ни одного порта**. UI ↔ ядро — Tauri IPC внутри процесса.
* Исходящие соединения: `127.0.0.1:11434` (Ollama; клиент отвергает нелокальные адреса, `Ollama::new` → `is_local_url`),
  облако только при явном включении (`ai.cloud_enabled`, `vision.allow_cloud`, `tts.cloud_enabled` — по умолчанию выключены),
  и загрузки мастера (только https с закреплённым SHA256, см. §7), запускаемые пользователем.
* Проверено на release (`scripts/e2e.ps1`, группы `headless` и `window`, 2026-11-18): `Get-NetTCPConnection -State Listen` и `-State Established`
  по дереву процессов — в трее слушающих портов нет, соединений с удалёнными хостами нет; в окне единственный слушающий порт — отладочный порт WebView2,
  который открывает сам тест (`--remote-debugging-port`), в обычном запуске его нет. Исходящее на `127.0.0.1:11434` появляется только при запросе к Ollama.

## 4. Секреты и логи

* Ключи (ElevenLabs, облачный LLM) — Windows Credential Manager (`keyring`), не в БД, не в настройках, не в логах.
* E2E после сценариев ищет в `logs/jarvis.log` и таблице `audit_log` шаблоны ключей (`api_key=…`, `sk-…`, `Bearer …`): совпадений нет.
* В аудит пишется имя секрета и событие, не значение. Текст команд пользователя хранится в `command_history` (память,
  срок хранения `privacy.history_days`, выключается `privacy.history_enabled`), в лог не попадает.

## 5. Права процесса

* Манифест `jarvis.manifest`: `requestedExecutionLevel = asInvoker`, `uiAccess = false`. Установщик NSIS — `installMode: currentUser`
  (без UAC; пишет только в `%LOCALAPPDATA%\Programs` и ярлык в меню Пуск текущего пользователя). Ни одна часть не требует прав администратора.
* Реестр: только ключ автозапуска `HKCU\...\Run` через настройку `general.autostart`; установщик реестр автозапуска не трогает.
* Дочерние процессы (wake-worker, gpu-worker, STT/Kokoro Python, Piper) запускаются без окна и привязаны к Job Object
  (kill-on-close): после выхода приложения сирот не остаётся.

## 6. Пути и файлы

* Файловые действия работают только внутри настроенных корней (`control.file_roots`), `files.open_folder` открывает только каталоги
  (исправлено в P6-A), удаление — только в корзину и с подтверждением.
* Скриншот (`vision.save_screenshot`) пишет один PNG в «Изображения»; остальные vision-действия не создают файлов.
* Распаковка архивов мастера (`setup::extract_zip`): `enclosed_name()` отвергает `..` и абсолютные пути (тест на zip-slip), распаковка
  во временную папку и атомарный перенос.
* Каталог данных задаётся `JARVIS_DATA_DIR`; тестовые запуски с этой переменной получают **отдельный идентификатор single-instance**
  (`instance_suffix`), чтобы не воздействовать на экземпляр пользователя.

## 7. Мастер первого запуска и установщик

* Загрузки: Vosk/Piper/Kokoro/голос — официальные URL (alphacephei.com, github.com/rhasspy, github.com/thewh1teagle, huggingface.co/rhasspy),
  принимаются **только при совпадении SHA256** (значения закреплены в `setup.rs`, повторяют `scripts/fetch-models.ps1`); несовпавший файл удаляется.
  Только `https://`. `uv.exe` — официальный релиз astral-sh/uv 0.13.8, SHA256 закреплён в `scripts/build-installer.ps1`.
  Whisper small скачивает `faster-whisper` через `huggingface_hub` (официальный репозиторий Systran; целостность проверяет сам hub, собственного SHA256 нет — см. «Остаётся»).
* Python-окружения — `uv sync --frozen` по `uv.lock` (версии и хэши колёс закреплены).
* Ollama не устанавливается и не запускается: мастер показывает инструкцию и ссылку; единственное действие — `ollama pull` через локальный API по кнопке.
* Деинсталлятор: по умолчанию данные и модели сохраняются; вопрос «удалить данные и модели?» с ответом «Нет» по умолчанию; тихий режим данные не удаляет.
* Тестовые хуки `JARVIS_E2E_TTS_WAV`, `JARVIS_E2E_VOICE_WAV`, `JARVIS_OLLAMA_URL` (только локальный адрес): работают, если переменная окружения
  процесса задана; тот, кто управляет окружением процесса, уже владеет им. Хуки не открывают портов и не принимают данных извне.

## 8. Инъекции через LLM

* LLM вызывает только зарегистрированные действия через `Executor::run` с валидацией аргументов и уровнями `safe|confirm|forbidden`;
  опасные действия требуют подтверждения пользователем (кнопка или голос; неоднозначный ответ никогда не подтверждает), по таймауту 30 с — отказ.
* Единственное «запоминающее» действие модели — `memory.remember` (Safe): это текст-заметка, в промпте помечается как данные, не как инструкции.
* OCR и VLM-описание экрана — данные: в промпт попадают как цитируемый материал, исполнять из них ничего нельзя (проверено тестами P9/P8).
* Ответ модели, не разбираемый как JSON-действие, превращается в безопасную фразу «Не понял, что сделать».

## 9. Найдено и исправлено в P10

1. `memory_facts` / `memory_history`: не было ограничения длины поискового текста → ≤ 200 символов (тест `overlong_search_text_is_refused`).
2. CSP не запрещала `object`, `base`, `form`, `frame` → добавлено.
3. Тестовый запуск с общим идентификатором single-instance мог сфокусировать/заменить окно рабочего экземпляра пользователя →
   идентификатор зависит от `JARVIS_DATA_DIR` (тест `isolated_runs_get_their_own_instance_identity`).
4. Паника в сервисе под Supervisor завершала задачу наблюдения без перезапуска (и в release с `panic = "abort"` — весь процесс) →
   сервис запускается в отдельной задаче, паника ловится, перезапуск с backoff, `FailureNotifier` → состояние WARNING и запись в аудит;
   профиль release переведён на `unwind`.
5. Worker STT при `JARVIS_MODELS_DIR` использовал корень моделей как папку Whisper → теперь `<root>/whisper` (согласовано с Rust-стороной).

## 10. Остаётся (с причиной)

* Whisper small без собственного SHA256: файлы тянет `huggingface_hub` с проверкой по LFS-хэшам Hugging Face; закрепить хэши можно после
  фиксации ревизии модели (отдельная задача).
* Установщик не подписан (нет сертификата): Windows SmartScreen предупредит при первом запуске.
* Единственный экземпляр: тестовые идентичности различаются по `JARVIS_DATA_DIR`; два экземпляра с одним каталогом данных не защищены от
  одновременной записи в SQLite кроме блокировок самой БД (WAL).

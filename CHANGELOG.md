# CHANGELOG

## [Unreleased]

## [0.2.0] — 2026-11-18 (первый публичный релиз)
- P13: оформленный двуязычный (RU/EN) установщик NSIS в стиле HUD (`installer-header.bmp`, `installer-sidebar.bmp` из `scripts/gen-installer-art.ps1`), выбор языка при запуске, страница лицензии Apache-2.0, ярлык на рабочем столе по желанию (по умолчанию выкл), запуск приложения на последней странице, деинсталлятор спрашивает об удалении данных/моделей (по умолчанию — оставить). Файл `JARVIS-Assistant_0.2.0_x64-setup.exe`. Версия 0.2.0 во всех манифестах, `LICENSE` (Apache-2.0).
- P12-UI: редизайн в стиле HUD (Inter + JetBrains Mono локально, бирюзовый акцент, скобки у панелей), колонки «Система» / «События», субтитры ядра, док ввода с PTT, настройки в выезжающей панели с иконками, `scripts/measure-window.ps1` (CPU окна по ролям процессов), скрипт скриншотов dev-сервера `app/scripts/screenshots.mjs`.

## Внутренний этап P10/P11 (до релиза 0.2.0)
- P10: e2e через WebView2 CDP (`scripts/e2e.ps1`), восстановление после сбоев (паника сервиса → перезапуск с backoff и WARNING; «локальная модель недоступна»),
  профили производительности (`scripts/perf.ps1`, `docs/PERFORMANCE.md`), аудит безопасности (`docs/SECURITY.md`).
- P11: установщик NSIS per-user (`scripts/build-installer.ps1`, `scripts/test-installer.ps1`), мастер первого запуска (вкладка SETUP), ресурсы относительно exe,
  `uv sync --frozen` для окружений воркеров, release-профиль `panic = unwind`.

## Внутренний этап Phase 2
- Phase 2: Tauri 2 приложение `jarvis.exe`: трей, режимы full/compact/tray (WebView уничтожается), single-instance, автозапуск `--tray`, Ctrl+Alt+J, Settings + журнал действий. Профиль трея: 8.8 МБ private, ~0% CPU.

## Внутренний этап Phase 1
- Phase 1: минимальный Rust Core `jarvisd` (EventBus, SQLite settings/audit, Supervisor, адаптивный монитор, selfstat), check.ps1, profile.ps1. Профиль 300 с: 7.5 МБ private, 0.01% CPU.

## Внутренний этап 0.0.2
- Ключевое слово по умолчанию — «Лёха» (список имён настраивается).
- Архитектура пересмотрена в Rust-first: постоянный процесс на Rust/Tauri, Python только по требованию (STT, Kokoro).
- API_CONTRACTS v0.2: Tauri IPC вместо WebSocket, бинарный протокол stdin/stdout для воркеров.

## Внутренний этап 0.0.1
- Созданы файлы управления проектом и контракты API v0.1.

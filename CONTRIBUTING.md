# Contributing to J.A.R.V.I.S.

Thanks for your interest. This is a Windows 11 desktop assistant: a Rust core, a Tauri 2 shell with a React + three.js UI and on-demand Python workers.
Read `AGENTS.md` (working rules), `ARCHITECTURE.md` and `API_CONTRACTS.md` first.

## Build

Requirements: Windows 11, Rust (stable, MSVC), Node.js 20+, [uv](https://docs.astral.sh/uv/), PowerShell 5.1+.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\fetch-models.ps1   # libvosk + the Vosk model (needed to build and test)
powershell -ExecutionPolicy Bypass -File scripts\stage-dev-resources.ps1   # bundle resources the Tauri crate needs to compile
npm ci --prefix app
powershell -ExecutionPolicy Bypass -File scripts\check.ps1          # everything below in one go
```

Run the app in development: `cd app; npx tauri dev`. Always point `JARVIS_DATA_DIR` at a scratch folder while developing
so that your real settings and memory are not touched. Release installer: `scripts\build-installer.ps1`.

## Checks (all must be green)

- Rust: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- Frontend (`app/`): `npm run typecheck`, `npm run lint`, `npm test`
- Workers (`workers/<name>/`): `uv run ruff check .`, `uv run ruff format --check .`, `uv run mypy .`, `uv run pytest` (slow model tests are opt-in: `-m slow`)

Tests that need real models or hardware are marked `#[ignore]` (Rust) or `slow` (Python).

## Rules for pull requests

- One topic per PR, small and reviewable. Describe what changed and why, and list the checks you ran.
- Keep the always-on process in Rust. Python runs only as an on-demand worker that exits when idle.
- No stubs, no `TODO` instead of logic, no fake system data, no commented-out tests.
- No new dependencies without a reason in the PR description; check that the license is compatible with Apache-2.0 and update `NOTICE`.
- Security: commands from voice or LLM output go through the action registry and risk levels; never add a way to run arbitrary shell commands.
- Privacy: everything stays local by default; cloud features must be opt-in and off by default.
- Do not commit secrets, personal paths, models or recordings. Tests must not touch the user's real data directory, and GUI tests must not drive the user's mouse or keyboard.
- Changes to the process budget (RAM/CPU) need a `scripts\profile.ps1` run; a regression above 20 % is not accepted.

By contributing you agree that your contribution is licensed under the Apache License 2.0.

"""JARVIS_MODELS_DIR is the shared models root; --download-only fetches the model."""

from __future__ import annotations

import sys
import types
from pathlib import Path

import pytest

from jarvis_stt import __main__ as entry
from jarvis_stt import engine


def test_models_dir_override_is_the_root_so_whisper_lives_in_a_subfolder(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("JARVIS_MODELS_DIR", str(tmp_path))
    assert engine.default_models_dir() == tmp_path / "whisper"


def test_download_only_fetches_the_model_and_does_not_serve(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    calls: list[tuple[str, str]] = []

    def fake_download(name: str, cache_dir: str) -> str:
        calls.append((name, cache_dir))
        return cache_dir

    module = types.ModuleType("faster_whisper.utils")
    module.download_model = fake_download  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "faster_whisper.utils", module)
    code = entry.main(["--download-only", "--models-dir", str(tmp_path), "--model", "small"])
    assert code == 0
    assert calls == [("small", str(tmp_path))]

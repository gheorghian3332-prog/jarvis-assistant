from __future__ import annotations

import sys
import types
from pathlib import Path

import numpy as np
import pytest

from jarvis_tts_kokoro import engine as engine_module
from jarvis_tts_kokoro.engine import SAMPLE_RATE, Audio, Engine, SynthesisError, language_for_voice, to_pcm16


class FakeModel:
    def __init__(self, rate: int = SAMPLE_RATE) -> None:
        self.rate = rate
        self.calls: list[tuple[str, str, float, str]] = []

    def create(self, text: str, voice: str, speed: float = 1.0, lang: str = "en-us") -> tuple[Audio, int]:
        self.calls.append((text, voice, speed, lang))
        return np.array([0.0, 0.5, -0.5, 1.5], dtype=np.float32), self.rate


def test_voice_prefix_selects_the_english_variant() -> None:
    assert language_for_voice("bm_george") == "en-gb"
    assert language_for_voice("bf_emma") == "en-gb"
    assert language_for_voice("am_adam") == "en-us"
    with pytest.raises(SynthesisError):
        language_for_voice("zz_nobody")


def test_pcm16_conversion_clips_and_is_little_endian() -> None:
    pcm = to_pcm16(np.array([0.0, 0.5, -0.5, 1.5, -2.0], dtype=np.float32))
    samples = np.frombuffer(pcm, dtype="<i2")
    assert samples.tolist() == [0, 16383, -16383, 32767, -32767]


def test_synthesize_passes_the_british_language_and_returns_pcm() -> None:
    model = FakeModel()
    result = Engine(model, "m.onnx").synthesize("  Good evening.  ", "bm_george", 1.1)
    assert model.calls == [("Good evening.", "bm_george", 1.1, "en-gb")]
    assert len(result.pcm) == 8


@pytest.mark.parametrize(
    ("text", "speed"),
    [("", 1.0), ("   ", 1.0), ("ok", 0.1), ("ok", 3.0), ("x" * 2_001, 1.0)],
)
def test_invalid_requests_are_rejected(text: str, speed: float) -> None:
    with pytest.raises(SynthesisError):
        Engine(FakeModel(), "m.onnx").synthesize(text, "bm_george", speed)


def test_unexpected_sample_rate_is_rejected() -> None:
    with pytest.raises(SynthesisError):
        Engine(FakeModel(rate=22_050), "m.onnx").synthesize("hello", "bm_george", 1.0)


def test_espeak_data_with_a_non_ascii_path_is_copied_to_an_ascii_folder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "папка" / "espeak-ng-data"
    source.mkdir(parents=True)
    (source / "phontab").write_bytes(b"x")
    fake = types.ModuleType("espeakng_loader")
    fake.get_data_path = lambda: str(source)  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "espeakng_loader", fake)
    models = tmp_path / "models"
    models.mkdir()
    result = engine_module.espeak_data_path(models)
    assert result.isascii()
    assert (Path(result) / "phontab").is_file()


def test_espeak_data_with_an_ascii_path_is_used_in_place(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "data"
    source.mkdir()
    fake = types.ModuleType("espeakng_loader")
    fake.get_data_path = lambda: str(source)  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "espeakng_loader", fake)
    assert engine_module.espeak_data_path(tmp_path) == str(source)

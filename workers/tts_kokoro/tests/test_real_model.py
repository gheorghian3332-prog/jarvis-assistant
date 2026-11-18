"""Loads the real Kokoro model; run with ``uv run pytest -m slow``."""

from __future__ import annotations

import numpy as np
import pytest

from jarvis_tts_kokoro import engine as engine_module


@pytest.mark.slow
def test_british_voice_speaks_a_phrase() -> None:
    engine = engine_module.load_engine(engine_module.default_models_dir())
    result = engine.synthesize("Good evening. All systems are running.", "bm_george", 1.0)
    samples = np.frombuffer(result.pcm, dtype="<i2")
    assert samples.size > engine_module.SAMPLE_RATE  # longer than one second
    assert np.abs(samples).max() > 3_000

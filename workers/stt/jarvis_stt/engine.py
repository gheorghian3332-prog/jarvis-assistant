"""faster-whisper wrapper: model loading with CUDA -> CPU fallback and transcription."""

from __future__ import annotations

import logging
import os
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

import numpy as np
import numpy.typing as npt

from . import cuda

log = logging.getLogger("jarvis_stt")

SAMPLE_RATE = 16_000
LANGUAGES = ("ru", "en")
DEFAULT_MODEL = "small"
COMPUTE_CUDA = "int8_float16"
COMPUTE_CPU = "int8"

Audio = npt.NDArray[np.float32]


class ModelLike(Protocol):
    def detect_language(self, audio: Audio, **kwargs: Any) -> tuple[str, float, list[tuple[str, float]]]: ...

    def transcribe(self, audio: Audio, **kwargs: Any) -> tuple[Any, Any]: ...


@dataclass(frozen=True)
class Result:
    text: str
    lang: str
    ms: int


def default_models_dir() -> Path:
    override = os.environ.get("JARVIS_MODELS_DIR")
    if override:
        return Path(override) / "whisper"
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / "Jarvis" / "models" / "whisper"


def pcm16_to_float(pcm: bytes) -> Audio:
    usable = len(pcm) - (len(pcm) % 2)
    samples = np.frombuffer(pcm[:usable], dtype="<i2")
    return (samples.astype(np.float32) / 32768.0).astype(np.float32)


def choose_language(probabilities: list[tuple[str, float]], hint: str | None) -> str:
    """Picks the language: the hint if it is supported, else the likelier of ru / en."""
    if hint in LANGUAGES:
        return str(hint)
    best = {lang: 0.0 for lang in LANGUAGES}
    for lang, probability in probabilities:
        if lang in best:
            best[lang] = float(probability)
    return max(LANGUAGES, key=lambda lang: best[lang])


class Engine:
    def __init__(self, model: ModelLike, device: str, compute_type: str, model_name: str) -> None:
        self._model = model
        self.device = device
        self.compute_type = compute_type
        self.model_name = model_name

    def transcribe(self, pcm: bytes, lang_hint: str | None = None) -> Result:
        started = time.perf_counter()
        audio = pcm16_to_float(pcm)
        if audio.size == 0:
            return Result(text="", lang=lang_hint if lang_hint in LANGUAGES else "ru", ms=0)
        if lang_hint in LANGUAGES:
            lang = str(lang_hint)
        else:
            _, _, probabilities = self._model.detect_language(audio)
            lang = choose_language(probabilities, None)
        segments, _info = self._model.transcribe(
            audio,
            language=lang,
            beam_size=1,
            vad_filter=False,
            condition_on_previous_text=False,
            without_timestamps=True,
        )
        text = " ".join(segment.text.strip() for segment in segments).strip()
        return Result(text=text, lang=lang, ms=int((time.perf_counter() - started) * 1000))


def _load(model_name: str, device: str, compute_type: str, models_dir: Path) -> ModelLike:
    from faster_whisper import WhisperModel

    models_dir.mkdir(parents=True, exist_ok=True)
    common: dict[str, Any] = {
        "device": device,
        "compute_type": compute_type,
        "download_root": str(models_dir),
    }
    try:
        # The model cache is used offline when possible; only the first start downloads weights.
        return WhisperModel(model_name, local_files_only=True, **common)  # type: ignore[no-any-return]
    except Exception:
        log.info("model %s is not cached; downloading from Hugging Face", model_name)
        return WhisperModel(model_name, **common)  # type: ignore[no-any-return]


def download_model(model_name: str, models_dir: Path) -> None:
    """Fetches the weights into the cache without loading them (the first-run wizard)."""
    from faster_whisper.utils import download_model as fetch

    models_dir.mkdir(parents=True, exist_ok=True)
    fetch(model_name, cache_dir=str(models_dir))


def _warm_up(model: ModelLike) -> None:
    """Runs one real inference so a broken CUDA runtime fails here, not on the first command."""
    silence = np.zeros(SAMPLE_RATE, dtype=np.float32)
    model.detect_language(silence)
    segments, _ = model.transcribe(silence, language="en", beam_size=1, vad_filter=False)
    for _segment in segments:
        pass


def load_engine(model_name: str, device: str, models_dir: Path) -> Engine:
    """``device`` is ``auto`` (CUDA, falling back to CPU), ``cuda`` or ``cpu``."""
    if device in ("auto", "cuda"):
        directories = cuda.add_dll_directories()
        log.info("nvidia dll directories: %s", [str(d) for d in directories])
        try:
            model = _load(model_name, "cuda", COMPUTE_CUDA, models_dir)
            _warm_up(model)
            return Engine(model, "cuda", COMPUTE_CUDA, model_name)
        except Exception as error:
            if device == "cuda":
                raise
            log.warning("CUDA unavailable (%s); falling back to CPU", error)
    model = _load(model_name, "cpu", COMPUTE_CPU, models_dir)
    _warm_up(model)
    return Engine(model, "cpu", COMPUTE_CPU, model_name)

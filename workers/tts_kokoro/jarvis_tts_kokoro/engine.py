"""Kokoro (kokoro-onnx) wrapper: model loading, voice/language mapping and PCM16 conversion."""

from __future__ import annotations

import logging
import os
import shutil
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

import numpy as np
import numpy.typing as npt

log = logging.getLogger("jarvis_tts_kokoro")

SAMPLE_RATE = 24_000
MIN_SPEED = 0.5
MAX_SPEED = 2.0
MAX_TEXT_CHARS = 2_000
DEFAULT_VOICE = "bm_george"
# Model files are looked up in this order inside the models folder.
MODEL_CANDIDATES = ("kokoro-v1.0.onnx", "kokoro-v1.0.int8.onnx")
VOICES_FILE = "voices-v1.0.bin"
# onnxruntime threads: enough for real time, few enough to leave the machine responsive.
MAX_THREADS = 4

Audio = npt.NDArray[np.float32]


class KokoroLike(Protocol):
    def create(self, text: str, voice: str, speed: float = 1.0, lang: str = "en-us") -> tuple[Audio, int]: ...


class SynthesisError(Exception):
    """The request cannot be synthesized (bad voice, bad speed, empty text)."""


@dataclass(frozen=True)
class Result:
    pcm: bytes
    ms: int


def default_models_dir() -> Path:
    override = os.environ.get("JARVIS_MODELS_DIR")
    if override:
        return Path(override) / "kokoro"
    base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
    return Path(base) / "Jarvis" / "models" / "kokoro"


def language_for_voice(voice: str) -> str:
    """British voices (``bm_``, ``bf_``) speak en-gb, American ones (``am_``, ``af_``) en-us."""
    if voice[:1] == "b":
        return "en-gb"
    if voice[:1] == "a":
        return "en-us"
    raise SynthesisError(f"unsupported voice: {voice}")


def to_pcm16(samples: Audio) -> bytes:
    clipped = np.clip(samples.astype(np.float32), -1.0, 1.0)
    return bytes((clipped * 32767.0).astype("<i2").tobytes())


class Engine:
    def __init__(self, model: KokoroLike, model_name: str) -> None:
        self._model = model
        self.model_name = model_name

    def synthesize(self, text: str, voice: str, speed: float) -> Result:
        text = text.strip()
        if not text:
            raise SynthesisError("empty text")
        if len(text) > MAX_TEXT_CHARS:
            raise SynthesisError(f"text is longer than {MAX_TEXT_CHARS} characters")
        if not MIN_SPEED <= speed <= MAX_SPEED:
            raise SynthesisError(f"speed must be between {MIN_SPEED} and {MAX_SPEED}")
        lang = language_for_voice(voice)
        started = time.perf_counter()
        try:
            samples, rate = self._model.create(text, voice=voice, speed=speed, lang=lang)
        except ValueError as error:
            raise SynthesisError(str(error)) from error
        if rate != SAMPLE_RATE:
            raise SynthesisError(f"unexpected sample rate {rate}")
        return Result(pcm=to_pcm16(samples), ms=int((time.perf_counter() - started) * 1000))


def find_model(models_dir: Path) -> Path:
    override = os.environ.get("JARVIS_KOKORO_MODEL")
    names = (override,) if override else MODEL_CANDIDATES
    for name in names:
        candidate = models_dir / name
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(f"no Kokoro model in {models_dir} (run scripts/fetch-models.ps1 -Kokoro)")


def espeak_data_path(models_dir: Path) -> str:
    """Folder with the espeak-ng data, guaranteed to have an ASCII-only path.

    espeak-ng opens its data with narrow-character file APIs, so it cannot read them from a folder
    whose path has non-ASCII characters (a repository under a Cyrillic folder name, for example).
    In that case the data is copied once next to the models (or into ProgramData).
    """
    import espeakng_loader

    source = Path(espeakng_loader.get_data_path())
    if str(source).isascii():
        return str(source)
    program_data = Path(os.environ.get("PROGRAMDATA", r"C:\ProgramData")) / "Jarvis"
    for root in (models_dir, program_data):
        target = root / "espeak-ng-data"
        if not str(target).isascii():
            continue
        if not (target / "phontab").is_file():
            log.info("copying espeak-ng data to %s", target)
            shutil.copytree(source, target, dirs_exist_ok=True)
        return str(target)
    raise RuntimeError("no ASCII-only folder available for the espeak-ng data")


def load_engine(models_dir: Path) -> Engine:
    import onnxruntime as rt
    from kokoro_onnx import Kokoro
    from kokoro_onnx.config import EspeakConfig

    model_path = find_model(models_dir)
    voices_path = models_dir / VOICES_FILE
    if not voices_path.is_file():
        raise FileNotFoundError(f"missing {voices_path} (run scripts/fetch-models.ps1 -Kokoro)")
    options = rt.SessionOptions()
    options.intra_op_num_threads = min(MAX_THREADS, os.cpu_count() or 1)
    session = rt.InferenceSession(str(model_path), options, providers=["CPUExecutionProvider"])
    model: Any = Kokoro.from_session(
        session, str(voices_path), espeak_config=EspeakConfig(data_path=espeak_data_path(models_dir))
    )
    engine = Engine(model, model_path.name)
    # One short phrase so that the first real request does not pay for graph optimization.
    engine.synthesize("Ready.", DEFAULT_VOICE, 1.0)
    return engine

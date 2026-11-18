"""Entry point: ``python -m jarvis_tts_kokoro``. Speaks the frame protocol on stdin/stdout."""

from __future__ import annotations

import argparse
import logging
import os
import sys
from pathlib import Path
from typing import IO

from . import engine as engine_module
from . import worker


def _claim_stdout() -> IO[bytes]:
    """Keeps the real stdout for frames and sends every stray write (also from C code) to stderr."""
    frames = os.fdopen(os.dup(1), "wb", buffering=0)
    os.dup2(2, 1)
    sys.stdout = sys.stderr
    return frames


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="jarvis_tts_kokoro")
    parser.add_argument("--models-dir", type=Path, default=None)
    args = parser.parse_args(argv)

    logging.basicConfig(
        stream=sys.stderr, level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
    )
    frames = _claim_stdout()
    models_dir = args.models_dir or engine_module.default_models_dir()
    try:
        engine = engine_module.load_engine(models_dir)
    except Exception:
        logging.getLogger("jarvis_tts_kokoro").exception("model load failed")
        return 2
    frames.write(worker.hello_frame(engine.model_name))
    return worker.serve(sys.stdin.buffer, frames, engine)


if __name__ == "__main__":
    sys.exit(main())

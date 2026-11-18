"""Entry point: ``python -m jarvis_stt``. Speaks the frame protocol on stdin/stdout."""

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


os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="jarvis_stt")
    parser.add_argument("--model", default=os.environ.get("JARVIS_STT_MODEL", engine_module.DEFAULT_MODEL))
    parser.add_argument(
        "--device",
        choices=("auto", "cuda", "cpu"),
        default=os.environ.get("JARVIS_STT_DEVICE", "auto"),
    )
    parser.add_argument("--models-dir", type=Path, default=None)
    parser.add_argument(
        "--download-only",
        action="store_true",
        help="fetch the model into the cache (first-run wizard) and exit",
    )
    args = parser.parse_args(argv)

    logging.basicConfig(
        stream=sys.stderr, level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s"
    )
    for noisy in ("httpx", "httpx2", "httpcore", "huggingface_hub"):
        logging.getLogger(noisy).setLevel(logging.WARNING)
    models_dir = args.models_dir or engine_module.default_models_dir()
    if args.download_only:
        try:
            engine_module.download_model(args.model, models_dir)
        except Exception:
            logging.getLogger("jarvis_stt").exception("model download failed")
            return 2
        return 0
    frames = _claim_stdout()
    try:
        engine = engine_module.load_engine(args.model, args.device, models_dir)
    except Exception:
        logging.getLogger("jarvis_stt").exception("model load failed")
        return 2
    frames.write(worker.hello_frame(engine.device, engine.model_name, engine.compute_type))
    return worker.serve(sys.stdin.buffer, frames, engine)


if __name__ == "__main__":
    sys.exit(main())

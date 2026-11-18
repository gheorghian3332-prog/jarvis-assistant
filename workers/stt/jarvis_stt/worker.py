"""Frame loop of the STT worker: stdin -> engine -> stdout."""

from __future__ import annotations

import logging
from typing import IO, Protocol

from . import protocol
from .engine import SAMPLE_RATE, Result

log = logging.getLogger("jarvis_stt")

WORKER_NAME = "stt"
WORKER_VERSION = "0.1.0"
# One utterance longer than this is rejected: a command phrase is a few seconds.
MAX_UTTERANCE_BYTES = SAMPLE_RATE * 2 * 120


class Transcriber(Protocol):
    def transcribe(self, pcm: bytes, lang_hint: str | None = None) -> Result: ...


def hello_frame(device: str, model: str, compute_type: str) -> bytes:
    return protocol.encode_json(
        protocol.HELLO,
        {
            "worker": WORKER_NAME,
            "version": WORKER_VERSION,
            "device": device,
            "model": model,
            "compute_type": compute_type,
        },
    )


def _error(code: str, message: str) -> bytes:
    return protocol.encode_json(protocol.ERROR, {"code": code, "message": message})


def _send(out: IO[bytes], data: bytes) -> None:
    out.write(data)
    out.flush()


def serve(stdin: IO[bytes], stdout: IO[bytes], engine: Transcriber) -> int:
    """Handles frames until SHUTDOWN or end of input. Returns the process exit code."""
    audio = bytearray()
    overflow = False
    while True:
        try:
            frame = protocol.read_frame(stdin)
        except protocol.TruncatedFrameError as error:
            log.warning("truncated frame: %s", error)
            return 0
        except protocol.ProtocolError as error:
            log.error("protocol error: %s", error)
            _send(stdout, _error("protocol", str(error)))
            return 1
        if frame is None:
            log.info("stdin closed; exiting")
            return 0
        if frame.type == protocol.SHUTDOWN:
            log.info("shutdown requested")
            return 0
        if frame.type == protocol.PING:
            _send(stdout, protocol.encode_frame(protocol.PONG, frame.payload))
        elif frame.type == protocol.AUDIO:
            if overflow or len(audio) + len(frame.payload) > MAX_UTTERANCE_BYTES:
                overflow = True
                audio.clear()
            else:
                audio.extend(frame.payload)
        elif frame.type == protocol.END_UTTERANCE:
            pcm = bytes(audio)
            audio.clear()
            if overflow:
                overflow = False
                _send(stdout, _error("audio_too_long", "utterance exceeds 120 seconds"))
                continue
            try:
                hint = protocol.decode_json(frame.payload).get("lang_hint")
                result = engine.transcribe(pcm, hint if isinstance(hint, str) else None)
            except protocol.ProtocolError as error:
                _send(stdout, _error("bad_request", str(error)))
                continue
            except Exception as error:
                log.exception("transcription failed")
                _send(stdout, _error("transcribe_failed", str(error)))
                continue
            _send(
                stdout,
                protocol.encode_json(
                    protocol.TRANSCRIPT,
                    {"text": result.text, "lang": result.lang, "final": True, "ms": result.ms},
                ),
            )
        else:
            _send(stdout, _error("unknown_type", f"unsupported frame type 0x{frame.type:02x}"))

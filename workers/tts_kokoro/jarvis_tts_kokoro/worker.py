"""Frame loop of the Kokoro worker: stdin -> engine -> stdout."""

from __future__ import annotations

import logging
from typing import IO, Protocol

from . import protocol
from .engine import Result, SynthesisError

log = logging.getLogger("jarvis_tts_kokoro")

WORKER_NAME = "tts_kokoro"
WORKER_VERSION = "0.1.0"


class Synthesizer(Protocol):
    def synthesize(self, text: str, voice: str, speed: float) -> Result: ...


def hello_frame(model: str, device: str = "cpu") -> bytes:
    return protocol.encode_json(
        protocol.HELLO,
        {"worker": WORKER_NAME, "version": WORKER_VERSION, "device": device, "model": model},
    )


def _error(code: str, message: str) -> bytes:
    return protocol.encode_json(protocol.ERROR, {"code": code, "message": message})


def _send(out: IO[bytes], data: bytes) -> None:
    out.write(data)
    out.flush()


def _request(payload: bytes) -> tuple[str, str, float]:
    body = protocol.decode_json(payload)
    text, voice, speed = body.get("text"), body.get("voice"), body.get("speed", 1.0)
    if not isinstance(text, str) or not isinstance(voice, str):
        raise protocol.ProtocolError("TTS_REQ needs string `text` and `voice`")
    if isinstance(speed, bool) or not isinstance(speed, int | float):
        raise protocol.ProtocolError("`speed` must be a number")
    return text, voice, float(speed)


def serve(stdin: IO[bytes], stdout: IO[bytes], engine: Synthesizer) -> int:
    """Handles frames until SHUTDOWN or end of input. Returns the process exit code."""
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
        elif frame.type == protocol.TTS_REQ:
            try:
                text, voice, speed = _request(frame.payload)
                result = engine.synthesize(text, voice, speed)
            except (protocol.ProtocolError, SynthesisError) as error:
                _send(stdout, _error("bad_request", str(error)))
                continue
            except Exception as error:
                log.exception("synthesis failed")
                _send(stdout, _error("synthesis_failed", str(error)))
                continue
            log.info("synthesized %d samples in %d ms", len(result.pcm) // 2, result.ms)
            _send(stdout, protocol.encode_frame(protocol.TTS_PCM, result.pcm))
        else:
            _send(stdout, _error("unknown_type", f"unsupported frame type 0x{frame.type:02x}"))

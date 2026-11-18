from __future__ import annotations

import io

from jarvis_tts_kokoro import protocol, worker
from jarvis_tts_kokoro.engine import Result, SynthesisError


class FakeEngine:
    def __init__(self) -> None:
        self.calls: list[tuple[str, str, float]] = []
        self.error: Exception | None = None

    def synthesize(self, text: str, voice: str, speed: float) -> Result:
        self.calls.append((text, voice, speed))
        if self.error is not None:
            raise self.error
        return Result(pcm=b"\x01\x00\x02\x00", ms=5)


def run(*frames: bytes, engine: FakeEngine | None = None) -> tuple[int, list[protocol.Frame]]:
    out = io.BytesIO()
    code = worker.serve(io.BytesIO(b"".join(frames)), out, engine or FakeEngine())
    out.seek(0)
    replies: list[protocol.Frame] = []
    while (frame := protocol.read_frame(out)) is not None:
        replies.append(frame)
    return code, replies


def request(text: str = "Hello", voice: str = "bm_george", speed: float = 1.0) -> bytes:
    return protocol.encode_json(protocol.TTS_REQ, {"text": text, "voice": voice, "speed": speed})


def test_shutdown_exits_cleanly_without_reply() -> None:
    code, replies = run(protocol.encode_frame(protocol.SHUTDOWN), protocol.encode_frame(protocol.PING))
    assert code == 0
    assert replies == []


def test_ping_is_answered_with_pong() -> None:
    _, replies = run(protocol.encode_frame(protocol.PING, b"7"))
    assert replies == [protocol.Frame(protocol.PONG, b"7")]


def test_request_returns_pcm() -> None:
    engine = FakeEngine()
    code, replies = run(request("Good evening, sir.", "bm_lewis", 1.2), engine=engine)
    assert code == 0
    assert engine.calls == [("Good evening, sir.", "bm_lewis", 1.2)]
    assert replies == [protocol.Frame(protocol.TTS_PCM, b"\x01\x00\x02\x00")]


def test_bad_request_is_reported_and_worker_continues() -> None:
    engine = FakeEngine()
    engine.error = SynthesisError("voice not found")
    code, replies = run(request(), protocol.encode_frame(protocol.PING, b"x"), engine=engine)
    assert code == 0
    assert replies[0].type == protocol.ERROR
    assert protocol.decode_json(replies[0].payload)["code"] == "bad_request"
    assert replies[1].type == protocol.PONG


def test_unexpected_failure_is_reported_as_synthesis_failed() -> None:
    engine = FakeEngine()
    engine.error = RuntimeError("boom")
    _, replies = run(request(), engine=engine)
    assert protocol.decode_json(replies[0].payload)["code"] == "synthesis_failed"


def test_malformed_request_body_is_rejected() -> None:
    for body in ({"voice": "bm_george"}, {"text": "x", "voice": "bm_george", "speed": "fast"}):
        _, replies = run(protocol.encode_json(protocol.TTS_REQ, body))
        assert replies[0].type == protocol.ERROR
        assert protocol.decode_json(replies[0].payload)["code"] == "bad_request"


def test_unknown_frame_type_is_reported() -> None:
    _, replies = run(protocol.encode_frame(0x55))
    assert protocol.decode_json(replies[0].payload)["code"] == "unknown_type"


def test_hello_frame_describes_the_worker() -> None:
    frame = protocol.read_frame(io.BytesIO(worker.hello_frame("kokoro-v1.0.onnx")))
    assert frame is not None
    body = protocol.decode_json(frame.payload)
    assert frame.type == protocol.HELLO
    assert body["worker"] == "tts_kokoro"
    assert body["model"] == "kokoro-v1.0.onnx"
    assert body["device"] == "cpu"

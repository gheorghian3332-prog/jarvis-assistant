from __future__ import annotations

import io

from jarvis_stt import protocol, worker
from jarvis_stt.engine import Result, choose_language, pcm16_to_float


class FakeEngine:
    def __init__(self) -> None:
        self.calls: list[tuple[int, str | None]] = []
        self.fail = False

    def transcribe(self, pcm: bytes, lang_hint: str | None = None) -> Result:
        self.calls.append((len(pcm), lang_hint))
        if self.fail:
            raise RuntimeError("boom")
        return Result(text="открой браузер", lang=lang_hint or "ru", ms=7)


def run(*frames: bytes, engine: FakeEngine | None = None) -> tuple[int, list[protocol.Frame]]:
    out = io.BytesIO()
    code = worker.serve(io.BytesIO(b"".join(frames)), out, engine or FakeEngine())
    out.seek(0)
    replies: list[protocol.Frame] = []
    while (frame := protocol.read_frame(out)) is not None:
        replies.append(frame)
    return code, replies


def test_shutdown_exits_cleanly_without_reply() -> None:
    code, replies = run(protocol.encode_frame(protocol.SHUTDOWN), protocol.encode_frame(protocol.PING))
    assert code == 0
    assert replies == []


def test_end_of_input_exits_cleanly() -> None:
    code, replies = run()
    assert code == 0
    assert replies == []


def test_ping_is_answered_with_pong() -> None:
    code, replies = run(protocol.encode_frame(protocol.PING, b"42"))
    assert code == 0
    assert replies == [protocol.Frame(protocol.PONG, b"42")]


def test_audio_chunks_are_joined_and_transcribed() -> None:
    engine = FakeEngine()
    _, replies = run(
        protocol.encode_frame(protocol.AUDIO, b"\x00\x01" * 100),
        protocol.encode_frame(protocol.AUDIO, b"\x00\x01" * 50),
        protocol.encode_json(protocol.END_UTTERANCE, {"lang_hint": "en"}),
        engine=engine,
    )
    assert engine.calls == [(300, "en")]
    assert len(replies) == 1
    assert replies[0].type == protocol.TRANSCRIPT
    body = protocol.decode_json(replies[0].payload)
    assert body == {"text": "открой браузер", "lang": "en", "final": True, "ms": 7}


def test_audio_buffer_is_reset_between_utterances() -> None:
    engine = FakeEngine()
    run(
        protocol.encode_frame(protocol.AUDIO, b"\x00\x00" * 10),
        protocol.encode_frame(protocol.END_UTTERANCE),
        protocol.encode_frame(protocol.AUDIO, b"\x00\x00" * 4),
        protocol.encode_frame(protocol.END_UTTERANCE),
        engine=engine,
    )
    assert engine.calls == [(20, None), (8, None)]


def test_unknown_type_gets_error_and_loop_continues() -> None:
    _, replies = run(protocol.encode_frame(0x55), protocol.encode_frame(protocol.PING))
    assert [r.type for r in replies] == [protocol.ERROR, protocol.PONG]
    assert protocol.decode_json(replies[0].payload)["code"] == "unknown_type"


def test_engine_failure_is_reported_and_worker_survives() -> None:
    engine = FakeEngine()
    engine.fail = True
    _, replies = run(
        protocol.encode_frame(protocol.END_UTTERANCE),
        protocol.encode_frame(protocol.PING),
        engine=engine,
    )
    assert [r.type for r in replies] == [protocol.ERROR, protocol.PONG]
    assert protocol.decode_json(replies[0].payload)["code"] == "transcribe_failed"


def test_bad_end_utterance_json_is_reported() -> None:
    _, replies = run(protocol.encode_frame(protocol.END_UTTERANCE, b"not json"))
    assert protocol.decode_json(replies[0].payload)["code"] == "bad_request"


def test_oversized_utterance_is_rejected_once() -> None:
    chunk = b"\x00" * (worker.MAX_UTTERANCE_BYTES // 2 + 2)
    engine = FakeEngine()
    _, replies = run(
        protocol.encode_frame(protocol.AUDIO, chunk),
        protocol.encode_frame(protocol.AUDIO, chunk),
        protocol.encode_frame(protocol.END_UTTERANCE),
        protocol.encode_frame(protocol.AUDIO, b"\x00\x00"),
        protocol.encode_frame(protocol.END_UTTERANCE),
        engine=engine,
    )
    codes = [protocol.decode_json(r.payload).get("code", "ok") for r in replies]
    assert codes == ["audio_too_long", "ok"]
    assert engine.calls == [(2, None)]


def test_truncated_frame_ends_worker_without_crash() -> None:
    raw = protocol.encode_frame(protocol.AUDIO, b"abcdef")[:-1]
    code, replies = run(raw)
    assert code == 0
    assert replies == []


def test_invalid_length_reports_protocol_error() -> None:
    code, replies = run(b"\x00\x00\x00\x00")
    assert code == 1
    assert protocol.decode_json(replies[0].payload)["code"] == "protocol"


def test_hello_frame_fields() -> None:
    frame = protocol.read_frame(io.BytesIO(worker.hello_frame("cuda", "small", "int8_float16")))
    assert frame is not None
    assert frame.type == protocol.HELLO
    body = protocol.decode_json(frame.payload)
    assert body["worker"] == "stt"
    assert body["device"] == "cuda"
    assert "version" in body


def test_choose_language_prefers_hint_then_ru_en() -> None:
    probabilities = [("de", 0.9), ("en", 0.04), ("ru", 0.05)]
    assert choose_language(probabilities, None) == "ru"
    assert choose_language(probabilities, "en") == "en"
    assert choose_language(probabilities, "de") == "ru"
    assert choose_language([], None) == "ru"


def test_pcm16_conversion_drops_odd_byte() -> None:
    audio = pcm16_to_float(b"\x00\x40\x00\xc0\x01")
    assert audio.tolist() == [0.5, -0.5]

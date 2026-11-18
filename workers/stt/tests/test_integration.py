"""Slow end-to-end test: real worker process, real Whisper model, real WAV phrases.

Run with ``uv run pytest -m slow -s``. The first run downloads the model into the model cache.
"""

from __future__ import annotations

import subprocess
import sys
import wave
from pathlib import Path

import pytest

from jarvis_stt import protocol

AUDIO = Path(__file__).parent / "audio"
CASES = [
    ("ru_open_browser.wav", "ru", ["браузер", "музык"]),
    ("en_open_browser.wav", "en", ["browser", "music"]),
]


def read_pcm(path: Path) -> bytes:
    with wave.open(str(path), "rb") as wav:
        assert wav.getframerate() == 16000
        assert wav.getnchannels() == 1
        assert wav.getsampwidth() == 2
        return wav.readframes(wav.getnframes())


def read_reply(process: subprocess.Popen[bytes]) -> protocol.Frame:
    assert process.stdout is not None
    frame = protocol.read_frame(process.stdout)
    assert frame is not None, "worker closed stdout"
    return frame


@pytest.mark.slow
def test_worker_recognizes_phrases() -> None:
    process = subprocess.Popen(
        [sys.executable, "-m", "jarvis_stt"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=None,
    )
    assert process.stdin is not None
    try:
        hello = read_reply(process)
        assert hello.type == protocol.HELLO
        info = protocol.decode_json(hello.payload)
        assert info["worker"] == "stt"
        assert info["device"] in ("cuda", "cpu")
        print("hello:", info)

        for name, lang, keywords in CASES:
            pcm = read_pcm(AUDIO / name)
            for offset in range(0, len(pcm), 6400):
                process.stdin.write(protocol.encode_frame(protocol.AUDIO, pcm[offset : offset + 6400]))
            process.stdin.write(protocol.encode_json(protocol.END_UTTERANCE, {}))
            process.stdin.flush()
            reply = read_reply(process)
            assert reply.type == protocol.TRANSCRIPT, reply
            body = protocol.decode_json(reply.payload)
            print(name, body)
            assert body["lang"] == lang
            assert body["final"] is True
            text = str(body["text"]).lower()
            for keyword in keywords:
                assert keyword in text, f"{keyword!r} not in {text!r}"

        process.stdin.write(protocol.encode_frame(protocol.PING))
        process.stdin.flush()
        assert read_reply(process).type == protocol.PONG

        process.stdin.write(protocol.encode_frame(protocol.SHUTDOWN))
        process.stdin.flush()
        assert process.wait(timeout=2) == 0
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()

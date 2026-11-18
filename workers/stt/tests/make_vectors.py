"""Regenerates tests/vectors/frames.bin, the byte vector shared with the Rust tests."""

from __future__ import annotations

from pathlib import Path

from jarvis_stt import protocol

FRAMES: list[tuple[int, bytes]] = [
    (protocol.PING, b""),
    (protocol.AUDIO, bytes([0x00, 0x01, 0x02, 0x03, 0xFF, 0x7F])),
    (protocol.END_UTTERANCE, b'{"lang_hint":"ru"}'),
    (protocol.TRANSCRIPT, '{"text":"привет","lang":"ru","final":true,"ms":123}'.encode()),
    (protocol.ERROR, b'{"code":"x","message":"y"}'),
    (protocol.SHUTDOWN, b""),
]


def build() -> bytes:
    return b"".join(protocol.encode_frame(frame_type, payload) for frame_type, payload in FRAMES)


if __name__ == "__main__":
    target = Path(__file__).parent / "vectors" / "frames.bin"
    target.write_bytes(build())
    print(f"wrote {target} ({target.stat().st_size} bytes)")

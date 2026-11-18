from __future__ import annotations

import io
from pathlib import Path

import pytest

from jarvis_stt import protocol
from tests import make_vectors

VECTOR = Path(__file__).parent / "vectors" / "frames.bin"


def test_round_trip_all_types() -> None:
    for frame_type in sorted(protocol.KNOWN_TYPES):
        for payload in (b"", b"x", bytes(range(256)) * 10):
            raw = protocol.encode_frame(frame_type, payload)
            frame = protocol.read_frame(io.BytesIO(raw))
            assert frame == protocol.Frame(frame_type, payload)


def test_length_counts_type_byte() -> None:
    raw = protocol.encode_frame(protocol.AUDIO, b"\x01\x02\x03")
    assert raw == bytes([0x04, 0x00, 0x00, 0x00, 0x10, 0x01, 0x02, 0x03])


def test_clean_end_of_stream_returns_none() -> None:
    assert protocol.read_frame(io.BytesIO(b"")) is None


def test_reads_consecutive_frames() -> None:
    stream = io.BytesIO(protocol.encode_frame(protocol.PING) + protocol.encode_frame(protocol.SHUTDOWN))
    assert protocol.read_frame(stream) == protocol.Frame(protocol.PING)
    assert protocol.read_frame(stream) == protocol.Frame(protocol.SHUTDOWN)
    assert protocol.read_frame(stream) is None


def test_truncated_length_prefix() -> None:
    with pytest.raises(protocol.TruncatedFrameError):
        protocol.read_frame(io.BytesIO(b"\x05\x00"))


def test_truncated_body() -> None:
    raw = protocol.encode_frame(protocol.AUDIO, b"abcdef")
    with pytest.raises(protocol.TruncatedFrameError):
        protocol.read_frame(io.BytesIO(raw[:-2]))


def test_zero_length_is_rejected() -> None:
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(io.BytesIO(b"\x00\x00\x00\x00"))


def test_oversized_length_is_rejected() -> None:
    huge = (protocol.MAX_FRAME_LEN + 1).to_bytes(4, "little")
    with pytest.raises(protocol.ProtocolError):
        protocol.read_frame(io.BytesIO(huge + b"\x10"))


def test_unknown_type_is_decoded_but_not_known() -> None:
    frame = protocol.read_frame(io.BytesIO(protocol.encode_frame(0x55, b"zz")))
    assert frame == protocol.Frame(0x55, b"zz")
    assert frame is not None
    assert frame.type not in protocol.KNOWN_TYPES


def test_json_helpers() -> None:
    raw = protocol.encode_json(protocol.TRANSCRIPT, {"text": "привет", "final": True})
    frame = protocol.read_frame(io.BytesIO(raw))
    assert frame is not None
    assert protocol.decode_json(frame.payload) == {"text": "привет", "final": True}
    assert protocol.decode_json(b"") == {}
    with pytest.raises(protocol.ProtocolError):
        protocol.decode_json(b"[1]")
    with pytest.raises(protocol.ProtocolError):
        protocol.decode_json(b"\xff\xfe")


def test_shared_vector_matches_encoder() -> None:
    assert VECTOR.read_bytes() == make_vectors.build()


def test_shared_vector_decodes() -> None:
    stream = io.BytesIO(VECTOR.read_bytes())
    decoded: list[tuple[int, bytes]] = []
    while (frame := protocol.read_frame(stream)) is not None:
        decoded.append((frame.type, frame.payload))
    assert decoded == make_vectors.FRAMES

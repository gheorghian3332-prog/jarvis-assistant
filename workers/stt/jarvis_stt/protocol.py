"""Binary frame codec shared with the Rust core (see API_CONTRACTS.md).

Frame layout: ``u32 LE len | u8 type | payload`` where ``len`` counts the type byte
plus the payload (so it is always >= 1).
"""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass
from typing import IO, Any

HELLO = 0x01
PING = 0x02
PONG = 0x03
AUDIO = 0x10
END_UTTERANCE = 0x11
TRANSCRIPT = 0x20
ERROR = 0x7E
SHUTDOWN = 0x7F

KNOWN_TYPES = frozenset({HELLO, PING, PONG, AUDIO, END_UTTERANCE, TRANSCRIPT, ERROR, SHUTDOWN})

# Upper bound for one frame (type + payload). 8 MiB is ~4 minutes of 16 kHz PCM16.
MAX_FRAME_LEN = 8 * 1024 * 1024

_LEN = struct.Struct("<I")


class ProtocolError(Exception):
    """The byte stream does not follow the frame format."""


class TruncatedFrameError(ProtocolError):
    """The stream ended in the middle of a frame."""


@dataclass(frozen=True)
class Frame:
    type: int
    payload: bytes = b""


def encode_frame(frame_type: int, payload: bytes = b"") -> bytes:
    if not 0 <= frame_type <= 0xFF:
        raise ValueError(f"frame type out of range: {frame_type}")
    length = 1 + len(payload)
    if length > MAX_FRAME_LEN:
        raise ProtocolError(f"frame too large: {length} bytes")
    return _LEN.pack(length) + bytes([frame_type]) + payload


def encode_json(frame_type: int, body: dict[str, Any]) -> bytes:
    return encode_frame(frame_type, json.dumps(body, ensure_ascii=False).encode("utf-8"))


def decode_json(payload: bytes) -> dict[str, Any]:
    if not payload:
        return {}
    try:
        value = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ProtocolError(f"invalid JSON payload: {error}") from error
    if not isinstance(value, dict):
        raise ProtocolError("JSON payload must be an object")
    return value


def _read_exact(stream: IO[bytes], size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining > 0:
        chunk = stream.read(remaining)
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def read_frame(stream: IO[bytes]) -> Frame | None:
    """Reads one frame. Returns None on a clean end of stream at a frame boundary."""
    header = _read_exact(stream, _LEN.size)
    if not header:
        return None
    if len(header) < _LEN.size:
        raise TruncatedFrameError("stream ended inside the length prefix")
    (length,) = _LEN.unpack(header)
    if length < 1:
        raise ProtocolError("frame length must be at least 1")
    if length > MAX_FRAME_LEN:
        raise ProtocolError(f"frame too large: {length} bytes")
    body = _read_exact(stream, length)
    if len(body) < length:
        raise TruncatedFrameError(f"stream ended inside a frame: {len(body)} of {length} bytes")
    return Frame(type=body[0], payload=body[1:])

"""Test double for the STT worker: speaks the frame protocol, no model, no GPU.

Not production code. Behaviour is selected with command line options so the Rust tests can
exercise timeouts, crashes and shutdown handling.
"""

import argparse
import json
import struct
import sys
import time
from pathlib import Path

HELLO, PING, PONG, AUDIO, END, TRANSCRIPT, ERROR, SHUTDOWN = 0x01, 0x02, 0x03, 0x10, 0x11, 0x20, 0x7E, 0x7F

parser = argparse.ArgumentParser()
parser.add_argument("--hello-delay", type=float, default=0.0)
parser.add_argument("--exit-before-hello", action="store_true")
parser.add_argument("--process-delay", type=float, default=0.0)
parser.add_argument("--ignore-shutdown", action="store_true")
parser.add_argument("--report-error", action="store_true")
parser.add_argument("--crash-once", type=Path, help="exit on the first utterance if this marker file is absent")
parser.add_argument("--hang-once", type=Path, help="never answer the first utterance if this marker is absent")
parser.add_argument("--pid-file", type=Path)
args = parser.parse_args()

stdin = sys.stdin.buffer
stdout = sys.stdout.buffer


def send(kind: int, payload: bytes = b"") -> None:
    stdout.write(struct.pack("<I", 1 + len(payload)) + bytes([kind]) + payload)
    stdout.flush()


def read_exact(size: int) -> bytes | None:
    data = b""
    while len(data) < size:
        chunk = stdin.read(size - len(data))
        if not chunk:
            return None
        data += chunk
    return data


def first_time(marker: Path | None) -> bool:
    if marker is None or marker.exists():
        return False
    marker.write_text("seen")
    return True


if args.pid_file:
    import os

    args.pid_file.write_text(str(os.getpid()))
print("fake worker starting", file=sys.stderr, flush=True)
time.sleep(args.hello_delay)
if args.exit_before_hello:
    print("fake worker: model load failed", file=sys.stderr, flush=True)
    sys.exit(3)
send(HELLO, json.dumps({"worker": "stt", "version": "fake", "device": "cpu", "model": "fake"}).encode())

audio = 0
while True:
    header = read_exact(4)
    if header is None:
        if args.ignore_shutdown:
            time.sleep(3600)
        sys.exit(0)
    (length,) = struct.unpack("<I", header)
    body = read_exact(length)
    if body is None:
        sys.exit(0)
    kind, payload = body[0], body[1:]
    if kind == SHUTDOWN:
        if args.ignore_shutdown:
            continue
        sys.exit(0)
    if kind == PING:
        send(PONG, payload)
    elif kind == AUDIO:
        audio += len(payload)
    elif kind == END:
        hint = json.loads(payload.decode() or "{}").get("lang_hint")
        received, audio = audio, 0
        if first_time(args.crash_once):
            print("fake worker: crashing", file=sys.stderr, flush=True)
            sys.exit(1)
        if first_time(args.hang_once):
            time.sleep(3600)
        time.sleep(args.process_delay)
        if args.report_error:
            send(ERROR, json.dumps({"code": "transcribe_failed", "message": "boom"}).encode())
        else:
            text = f"fake {received} bytes"
            send(TRANSCRIPT, json.dumps({"text": text, "lang": hint or "ru", "final": True, "ms": 1}).encode())
    elif kind == 0x30:
        # TTS_REQ -> TTS_PCM: one sample of value 1000 per character of the text.
        request = json.loads(payload.decode())
        if request["text"] == "fail":
            send(ERROR, json.dumps({"code": "bad_request", "message": "no such voice"}).encode())
        else:
            send(0x31, struct.pack("<h", 1000) * len(request["text"]))
    else:
        send(ERROR, json.dumps({"code": "unknown_type", "message": hex(kind)}).encode())

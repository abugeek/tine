"""Minimal client for the running tine.exe control pipe (\\\\.\\pipe\\tine-<sid>).

    python -I tine_pipe.py status        -> prints the JSON response
    python -I tine_pipe.py quit
    python -I tine_pipe.py open          -> opens the UI window (spawns tine.exe --ui)
    python -I tine_pipe.py monitor on|off
    python -I tine_pipe.py raw '{"cmd":"monitor","on":false}'
"""

from __future__ import annotations

import json
import os
import sys

import pywintypes
import win32file
import win32pipe


def pipe_path() -> str | None:
    try:
        names = os.listdir("\\\\.\\pipe\\")
    except OSError:
        return None
    for n in names:
        if n.lower().startswith("tine-"):
            return "\\\\.\\pipe\\" + n
    return None


def request(req: dict, timeout_ms: int = 5000) -> dict:
    path = pipe_path()
    if path is None:
        return {"ok": False, "error": "tine is not running (no control pipe)"}
    try:
        win32pipe.WaitNamedPipe(path, timeout_ms)
        h = win32file.CreateFile(
            path, win32file.GENERIC_READ | win32file.GENERIC_WRITE, 0, None,
            win32file.OPEN_EXISTING, 0, None,
        )
    except pywintypes.error as e:
        return {"ok": False, "error": f"open pipe: {e}"}
    try:
        win32file.WriteFile(h, (json.dumps(req) + "\n").encode())
        buf = b""
        while b"\n" not in buf:
            try:
                _, chunk = win32file.ReadFile(h, 65536)
            except pywintypes.error:
                break
            if not chunk:
                break
            buf += chunk
        line = buf.split(b"\n", 1)[0]
        return json.loads(line) if line else {"ok": True, "data": None}
    finally:
        win32file.CloseHandle(h)


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    cmd = sys.argv[1]
    # Plain-word forms exist because Windows PowerShell 5.1 strips the double
    # quotes from JSON arguments passed to native programs.
    if cmd == "raw":
        req = json.loads(sys.argv[2])
    elif cmd == "monitor":
        req = {"cmd": "monitor", "on": sys.argv[2].lower() in ("on", "true", "1"), "device": None}
    else:
        req = {"cmd": cmd}
    print(json.dumps(request(req)))
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""Dropout / click detector for a recorded virtual-mic stream.

Reads a 32-bit float (or 16-bit PCM) WAV recorded from an app's virtual
microphone (e.g. `CABLE Output` for Tine, `SteelSeries Sonar - Microphone`)
and reports signs of audio glitches:

* dropouts  - runs of exact digital zero >= --min-gap-ms that sit *between*
              non-zero audio. A processed live mic almost never produces exact
              0.0 for a millisecond on its own; a starved stream does.
* clicks    - sample-to-sample jumps far outside the stream's normal motion
              (robust median/MAD threshold), the signature of a discontinuity.

These are heuristics: compare the same app's numbers between an unloaded and
a loaded run (the delta is the signal), not one app's absolute count.

    python -I audio_check.py rec.wav [--min-gap-ms 1.0] [--json out.json]
"""

from __future__ import annotations

import argparse
import json
import struct
import sys

import numpy as np


def read_wav(path: str) -> tuple[np.ndarray, int]:
    with open(path, "rb") as fh:
        data = fh.read()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise ValueError("not a RIFF/WAVE file")
    pos, fmt, raw = 12, None, None
    while pos + 8 <= len(data):
        cid, size = data[pos:pos + 4], struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + size]
        if cid == b"fmt ":
            tag, ch, rate, _, _, bits = struct.unpack("<HHIIHH", body[:16])
            if tag == 0xFFFE and len(body) >= 26:  # WAVE_FORMAT_EXTENSIBLE
                tag = struct.unpack("<H", body[24:26])[0]
            fmt = (tag, ch, rate, bits)
        elif cid == b"data":
            raw = body
        pos += 8 + size + (size & 1)
    if fmt is None or raw is None:
        raise ValueError("missing fmt/data chunk")
    tag, ch, rate, bits = fmt
    if tag == 3 and bits == 32:
        x = np.frombuffer(raw[: len(raw) // 4 * 4], dtype="<f4").astype(np.float64)
    elif tag == 1 and bits == 16:
        x = np.frombuffer(raw[: len(raw) // 2 * 2], dtype="<i2").astype(np.float64) / 32768.0
    else:
        raise ValueError(f"unsupported WAV format tag={tag} bits={bits}")
    if ch > 1:
        x = x[: len(x) // ch * ch].reshape(-1, ch).mean(axis=1)
    return x, rate


def zero_runs(x: np.ndarray) -> list[tuple[int, int]]:
    """(start, length) of every maximal run of exact zeros strictly inside x."""
    z = np.concatenate(([0], (x == 0.0).astype(np.int8), [0]))
    d = np.diff(z)
    starts, ends = np.flatnonzero(d == 1), np.flatnonzero(d == -1)
    return [(int(s), int(e - s)) for s, e in zip(starts, ends) if s > 0 and e < len(x)]


def analyse(path: str, min_gap_ms: float) -> dict:
    x, rate = read_wav(path)
    n = len(x)
    if n == 0:
        return {"file": path, "error": "empty recording"}
    nz = np.flatnonzero(x != 0.0)
    if nz.size == 0:
        return {"file": path, "rate_hz": rate, "seconds": n / rate, "error": "all-zero recording"}
    # ignore start-up / tail silence plus 100 ms of stream priming at each end
    edge = int(rate * 0.1)
    body = x[nz[0] + edge: nz[-1] + 1 - edge]
    if body.size == 0:
        return {"file": path, "rate_hz": rate, "seconds": n / rate, "error": "recording too short"}
    min_gap = max(1, int(rate * min_gap_ms / 1e3))
    # A gate fading to zero is legitimate; a stream starving mid-signal is not.
    # Only count gaps with clearly audible audio (> -60 dBFS RMS over 2 ms)
    # immediately before AND after.
    win = max(1, int(rate * 0.002))
    floor = 10 ** (-60 / 20)

    def loud(seg: np.ndarray) -> bool:
        return seg.size > 0 and float(np.sqrt(np.mean(seg**2))) > floor

    gaps = [
        (s, l)
        for s, l in zero_runs(body)
        if l >= min_gap and loud(body[max(0, s - win): s]) and loud(body[s + l: s + l + win])
    ]
    audible_pct = 0.0
    if len(body) >= win:
        frames = body[: len(body) // win * win].reshape(-1, win)
        audible_pct = 100.0 * float(np.mean(np.sqrt(np.mean(frames**2, axis=1)) > floor))
    d = np.abs(np.diff(body))
    med = float(np.median(d))
    mad = float(np.median(np.abs(d - med))) + 1e-12
    thresh = max(med + 40.0 * 1.4826 * mad, 0.05)
    clicks = int(np.count_nonzero(d > thresh))
    rms = float(np.sqrt(np.mean(body**2)))
    peak = float(np.max(np.abs(body)))
    return {
        "file": path,
        "rate_hz": rate,
        "seconds": round(n / rate, 3),
        "active_seconds": round(len(body) / rate, 3),
        "rms_dbfs": round(20 * np.log10(rms + 1e-12), 2),
        "peak_dbfs": round(20 * np.log10(peak + 1e-12), 2),
        "exact_zero_pct": round(100.0 * float(np.mean(body == 0.0)), 3),
        "audible_pct": round(audible_pct, 2),
        "dropouts": len(gaps),
        "dropout_total_ms": round(sum(l for _, l in gaps) / rate * 1e3, 2),
        "dropout_longest_ms": round(max((l for _, l in gaps), default=0) / rate * 1e3, 2),
        "dropouts_at_s": [round(s / rate, 3) for s, _ in gaps[:20]],
        "clicks": clicks,
        "click_threshold": round(thresh, 5),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("wav")
    ap.add_argument("--min-gap-ms", type=float, default=1.0)
    ap.add_argument("--json")
    a = ap.parse_args()
    try:
        out = analyse(a.wav, a.min_gap_ms)
    except (OSError, ValueError) as e:
        out = {"file": a.wav, "error": str(e)}
    js = json.dumps(out)
    print(js)
    if a.json:
        with open(a.json, "w", encoding="utf-8") as fh:
            fh.write(js)
    return 0


if __name__ == "__main__":
    sys.exit(main())

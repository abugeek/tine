"""Game-like CPU load + frame-pacing probe.

* N worker processes (default: every logical core) spin on arithmetic at normal
  priority, like a CPU-bound game. Their combined throughput (work units / s)
  is the "game performance" number: if a background app steals CPU, it drops.
* One probe process runs a 144 Hz "frame loop": each frame does ~1 ms of fixed
  work, then sleeps until the next deadline. It records how late every frame
  wakes up. Late frames are what a player feels as stutter.

Writes a JSON summary to --report and prints it.

    python -I cpu_stress.py --seconds 60 [--workers N] [--hz 144] --report out.json
"""

from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import os
import statistics
import time


def worker(counter, stop_at: float) -> None:
    n = 0
    x = 1.0001
    while True:
        for _ in range(20000):
            x = (x * 1.0000001 + 0.5) % 1000.0
        n += 1
        if n % 16 == 0:
            with counter.get_lock():
                counter.value += 16
            if time.perf_counter() >= stop_at:
                return


def probe(hz: float, stop_at: float, q) -> None:
    period = 1.0 / hz
    next_t = time.perf_counter() + period
    late_ms: list[float] = []
    frame_ms: list[float] = []
    prev_wake = None
    x = 1.0
    while True:
        # ~1 ms of fixed "game logic" per frame
        end = time.perf_counter() + 0.001
        while time.perf_counter() < end:
            x = (x * 1.0000001 + 0.5) % 1000.0
        delay = next_t - time.perf_counter()
        if delay > 0:
            time.sleep(delay)
        wake = time.perf_counter()
        late_ms.append(max(0.0, (wake - next_t) * 1e3))
        if prev_wake is not None:
            frame_ms.append((wake - prev_wake) * 1e3)
        prev_wake = wake
        next_t += period
        if wake - next_t > 0.1:  # badly behind: resync instead of bursting
            next_t = wake + period
        if wake >= stop_at:
            break
    q.put((late_ms, frame_ms))


def pct(v: list[float], p: float) -> float:
    if not v:
        return 0.0
    s = sorted(v)
    return s[min(len(s) - 1, int(round(p / 100.0 * (len(s) - 1))))]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=60.0)
    ap.add_argument("--workers", type=int, default=os.cpu_count() or 4)
    ap.add_argument("--hz", type=float, default=144.0)
    ap.add_argument("--report")
    ap.add_argument("--no-probe", action="store_true")
    args = ap.parse_args()

    counter = mp.Value("Q", 0)
    q: mp.Queue = mp.Queue()
    stop_at = time.perf_counter() + args.seconds
    procs = [mp.Process(target=worker, args=(counter, stop_at)) for _ in range(args.workers)]
    for p in procs:
        p.start()
    pr = None
    if not args.no_probe:
        pr = mp.Process(target=probe, args=(args.hz, stop_at, q))
        pr.start()

    # throughput per second, skipping the first second (spin-up)
    rates: list[float] = []
    last_v, last_t = counter.value, time.perf_counter()
    while time.perf_counter() < stop_at:
        time.sleep(1.0)
        v, t = counter.value, time.perf_counter()
        rates.append((v - last_v) / (t - last_t))
        last_v, last_t = v, t
    late, frames = q.get() if pr else ([], [])
    for p in procs:
        p.join()
    if pr:
        pr.join()

    budget = 1e3 / args.hz
    steady = rates[1:] if len(rates) > 2 else rates
    out = {
        "workers": args.workers,
        "seconds": args.seconds,
        "work_units_per_s_mean": round(statistics.fmean(steady), 2) if steady else 0.0,
        "work_units_per_s_stdev": round(statistics.pstdev(steady), 2) if steady else 0.0,
        "probe_hz": args.hz,
        "frames": len(frames),
        "late_ms_p50": round(pct(late, 50), 3),
        "late_ms_p99": round(pct(late, 99), 3),
        "late_ms_p999": round(pct(late, 99.9), 3),
        "late_ms_max": round(max(late, default=0.0), 3),
        "frames_late_over_2ms": sum(1 for v in late if v > 2.0),
        "frame_ms_p99": round(pct(frames, 99), 3),
        "frames_over_budget_x1_5": sum(1 for v in frames if v > budget * 1.5),
    }
    js = json.dumps(out)
    print(js)
    if args.report:
        with open(args.report, "w", encoding="utf-8") as fh:
            fh.write(js)
    return 0


if __name__ == "__main__":
    mp.freeze_support()
    raise SystemExit(main())

"""Per-app resource sampler for the Tine vs. competitor benchmark.

Samples every process of each app group once per period and writes one CSV row
per (sample, group):

    t_s, phase, group, procs, cpu_pct, cpu_pct_max_proc, working_set_mb,
    private_mb, threads, ctx_switches_per_s, gpu_pct, gpu_dedicated_mb,
    gpu_shared_mb, sys_cpu_pct

* cpu_pct is percent of ONE logical core (100 = one core fully busy), summed
  over the group's processes - the same unit as Task Manager's "Details" tab
  times the core count. Divide by the logical-core count for "share of the
  whole CPU".
* gpu_pct mirrors Task Manager: per engine type, sum the group's engine
  utilisation; report the busiest engine type.
* The current phase is read from a small text file the orchestrator rewrites,
  so one sampler runs across every scenario.

Usage:
    python -I sampler.py --out samples.csv --phase-file phase.txt \
        --group tine=tine.exe --group sonar=SteelSeries* --group audiodg=audiodg.exe \
        [--period 1.0] [--stop-file stop.txt]
"""

from __future__ import annotations

import argparse
import csv
import fnmatch
import os
import re
import sys
import time
from collections import defaultdict

import psutil
import win32pdh

PID_RE = re.compile(r"pid_(\d+)_")
ENGTYPE_RE = re.compile(r"engtype_([A-Za-z0-9 ]+)$")


class GpuCounters:
    """Wildcard PDH counters for per-process GPU engine use and GPU memory."""

    def __init__(self) -> None:
        self.query = win32pdh.OpenQuery()
        self.engine = win32pdh.AddEnglishCounter(
            self.query, r"\GPU Engine(*)\Utilization Percentage"
        )
        self.dedicated = win32pdh.AddEnglishCounter(
            self.query, r"\GPU Process Memory(*)\Dedicated Usage"
        )
        self.shared = win32pdh.AddEnglishCounter(
            self.query, r"\GPU Process Memory(*)\Shared Usage"
        )
        win32pdh.CollectQueryData(self.query)  # prime the rate counters

    @staticmethod
    def _array(counter) -> dict:
        try:
            return win32pdh.GetFormattedCounterArray(counter, win32pdh.PDH_FMT_DOUBLE)
        except Exception:  # noqa: BLE001 - instances can vanish between collects
            return {}

    def collect(self):
        """Returns ({pid: {engtype: pct}}, {pid: dedicated_bytes}, {pid: shared_bytes})."""
        try:
            win32pdh.CollectQueryData(self.query)
        except Exception:  # noqa: BLE001
            return {}, {}, {}
        eng: dict[int, dict[str, float]] = defaultdict(lambda: defaultdict(float))
        for inst, val in self._array(self.engine).items():
            m, e = PID_RE.search(inst), ENGTYPE_RE.search(inst)
            if m and e:
                eng[int(m.group(1))][e.group(1)] += val
        ded: dict[int, float] = defaultdict(float)
        for inst, val in self._array(self.dedicated).items():
            if m := PID_RE.search(inst):
                ded[int(m.group(1))] += val
        sh: dict[int, float] = defaultdict(float)
        for inst, val in self._array(self.shared).items():
            if m := PID_RE.search(inst):
                sh[int(m.group(1))] += val
        return eng, ded, sh


def matches(name: str, patterns: list[str]) -> bool:
    low = name.lower()
    return any(fnmatch.fnmatch(low, p.lower()) for p in patterns)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--phase-file", required=True)
    ap.add_argument("--stop-file")
    ap.add_argument("--period", type=float, default=1.0)
    ap.add_argument(
        "--group",
        action="append",
        required=True,
        help="name=glob[,glob...] (process image names, case-insensitive)",
    )
    args = ap.parse_args()

    groups: dict[str, list[str]] = {}
    for g in args.group:
        name, _, globs = g.partition("=")
        groups[name] = [p.strip() for p in globs.split(",") if p.strip()]

    gpu = GpuCounters()
    psutil.cpu_percent(None)
    # pid -> (cpu_seconds, ctx_switches) from the previous sample
    prev: dict[int, tuple[float, int]] = {}
    last = time.perf_counter()
    start = last

    fields = [
        "t_s", "phase", "group", "procs", "cpu_pct", "cpu_pct_max_proc",
        "working_set_mb", "private_mb", "threads", "ctx_switches_per_s",
        "gpu_pct", "gpu_dedicated_mb", "gpu_shared_mb", "sys_cpu_pct",
    ]
    with open(args.out, "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=fields)
        w.writeheader()
        while True:
            if args.stop_file and os.path.exists(args.stop_file):
                break
            time.sleep(max(0.0, args.period - (time.perf_counter() - last)))
            now = time.perf_counter()
            dt = max(now - last, 1e-3)
            last = now
            try:
                with open(args.phase_file, encoding="utf-8") as pf:
                    phase = pf.read().strip() or "unknown"
            except OSError:
                phase = "unknown"

            eng, ded, sh = gpu.collect()
            sys_cpu = psutil.cpu_percent(None)
            rows = {g: defaultdict(float) for g in groups}
            per_proc_cpu = {g: [] for g in groups}
            per_group_eng = {g: defaultdict(float) for g in groups}
            seen = set()
            for p in psutil.process_iter(["name"]):
                name = p.info.get("name") or ""
                grp = next((g for g, pats in groups.items() if matches(name, pats)), None)
                if grp is None:
                    continue
                try:
                    with p.oneshot():
                        ct = p.cpu_times()
                        cpu_s = ct.user + ct.system
                        mi = p.memory_info()
                        ctx = p.num_ctx_switches()
                        ctx_total = ctx.voluntary + ctx.involuntary
                        threads = p.num_threads()
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    continue
                seen.add(p.pid)
                pc, pctx = prev.get(p.pid, (cpu_s, ctx_total))
                cpu_pct = max(0.0, (cpu_s - pc) / dt * 100.0)
                ctx_rate = max(0.0, (ctx_total - pctx) / dt)
                prev[p.pid] = (cpu_s, ctx_total)
                r = rows[grp]
                r["procs"] += 1
                r["cpu_pct"] += cpu_pct
                r["working_set_mb"] += mi.wset / 2**20
                r["private_mb"] += getattr(mi, "private", mi.vms) / 2**20
                r["threads"] += threads
                r["ctx_switches_per_s"] += ctx_rate
                r["gpu_dedicated_mb"] += ded.get(p.pid, 0.0) / 2**20
                r["gpu_shared_mb"] += sh.get(p.pid, 0.0) / 2**20
                per_proc_cpu[grp].append(cpu_pct)
                for et, v in eng.get(p.pid, {}).items():
                    per_group_eng[grp][et] += v
            for pid in list(prev):
                if pid not in seen:
                    del prev[pid]
            t = round(now - start, 3)
            for g, r in rows.items():
                w.writerow({
                    "t_s": t,
                    "phase": phase,
                    "group": g,
                    "procs": int(r["procs"]),
                    "cpu_pct": round(r["cpu_pct"], 3),
                    "cpu_pct_max_proc": round(max(per_proc_cpu[g], default=0.0), 3),
                    "working_set_mb": round(r["working_set_mb"], 2),
                    "private_mb": round(r["private_mb"], 2),
                    "threads": int(r["threads"]),
                    "ctx_switches_per_s": round(r["ctx_switches_per_s"], 1),
                    "gpu_pct": round(max(per_group_eng[g].values(), default=0.0), 3),
                    "gpu_dedicated_mb": round(r["gpu_dedicated_mb"], 2),
                    "gpu_shared_mb": round(r["gpu_shared_mb"], 2),
                    "sys_cpu_pct": round(sys_cpu, 2),
                })
            fh.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())

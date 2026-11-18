"""Aggregates a compare.ps1 run folder into summary.json and summary.md.

    python -I report.py <run-dir>
"""

from __future__ import annotations

import csv
import glob
import json
import os
import statistics
import sys
from collections import defaultdict

SKIP_PHASES = {"warmup", "between", "start", "unknown"}
METRICS = [
    "cpu_pct", "working_set_mb", "private_mb", "threads",
    "ctx_switches_per_s", "gpu_pct", "gpu_dedicated_mb", "gpu_shared_mb", "procs",
]


def pct(v: list[float], p: float) -> float:
    if not v:
        return 0.0
    s = sorted(v)
    return s[min(len(s) - 1, int(round(p / 100 * (len(s) - 1))))]


def load_json(path: str):
    try:
        with open(path, encoding="utf-8-sig") as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def main() -> int:
    run = sys.argv[1]
    meta = load_json(os.path.join(run, "meta.json")) or {}
    rows: dict[tuple[str, str], dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
    sys_cpu: dict[str, list[float]] = defaultdict(list)
    order: list[str] = []
    with open(os.path.join(run, "samples.csv"), encoding="utf-8") as fh:
        for r in csv.DictReader(fh):
            ph = r["phase"]
            if ph in SKIP_PHASES:
                continue
            if ph not in order:
                order.append(ph)
            for m in METRICS + ["cpu_pct_max_proc"]:
                rows[(ph, r["group"])][m].append(float(r[m]))
            if r["group"] == "tine":
                sys_cpu[ph].append(float(r["sys_cpu_pct"]))

    logical = (meta.get("machine") or {}).get("logical") or os.cpu_count() or 1
    phases: dict[str, dict] = {}
    for ph in order:
        groups = {}
        for (p, g), m in rows.items():
            if p != ph:
                continue
            cpu = m["cpu_pct"]
            groups[g] = {
                "samples": len(cpu),
                "procs": max(m["procs"], default=0),
                "cpu_pct_core_mean": round(statistics.fmean(cpu), 3) if cpu else 0.0,
                "cpu_pct_core_p95": round(pct(cpu, 95), 3),
                "cpu_pct_core_max": round(max(cpu, default=0.0), 3),
                "cpu_pct_total_mean": round(statistics.fmean(cpu) / logical, 4) if cpu else 0.0,
                "working_set_mb_mean": round(statistics.fmean(m["working_set_mb"]), 1),
                "working_set_mb_max": round(max(m["working_set_mb"]), 1),
                "private_mb_mean": round(statistics.fmean(m["private_mb"]), 1),
                "private_mb_max": round(max(m["private_mb"]), 1),
                "threads_mean": round(statistics.fmean(m["threads"]), 1),
                "ctx_switches_per_s_mean": round(statistics.fmean(m["ctx_switches_per_s"]), 1),
                "gpu_pct_mean": round(statistics.fmean(m["gpu_pct"]), 3),
                "gpu_pct_max": round(max(m["gpu_pct"]), 3),
                "gpu_dedicated_mb_max": round(max(m["gpu_dedicated_mb"]), 1),
                "gpu_shared_mb_max": round(max(m["gpu_shared_mb"]), 1),
            }
        extra = {}
        for f in sorted(glob.glob(os.path.join(run, f"{ph}.*.json"))):
            key = os.path.basename(f)[len(ph) + 1: -len(".json")]
            extra[key] = load_json(f)
        phases[ph] = {
            "system_cpu_pct_mean": round(statistics.fmean(sys_cpu[ph]), 2) if sys_cpu[ph] else None,
            "groups": groups,
            "artifacts": extra,
        }

    summary = {"meta": meta, "phases": phases}
    with open(os.path.join(run, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump(summary, fh, indent=1)

    comp = (meta.get("competitor") or {}).get("name", "competitor")
    out = [f"# Tine vs {comp} - resource benchmark", ""]
    mach = meta.get("machine") or {}
    out += [f"- Machine: {mach.get('cpu')} ({logical} threads), {mach.get('ram_gb')} GB RAM, {mach.get('gpu')}",
            f"- OS: {mach.get('os')}", f"- Phase length: {meta.get('phase_seconds')} s (+ warmup)", "",
            "CPU is % of one core (100 = one core busy); total-CPU share = that / logical threads.", ""]
    hdr = "| phase | app | procs | CPU % core (mean / p95) | CPU % of total | WS MB | private MB | threads | ctx sw/s | GPU % (mean/max) | VRAM MB |"
    out += [hdr, "|" + "---|" * 11]
    for ph in order:
        for g in ("tine", comp, "audiodg"):
            s = phases[ph]["groups"].get(g)
            if not s or (s["procs"] == 0 and g == "tine"):
                continue
            out.append(
                f"| {ph} | {g} | {s['procs']:.0f} | {s['cpu_pct_core_mean']:.2f} / {s['cpu_pct_core_p95']:.2f} | "
                f"{s['cpu_pct_total_mean']:.3f} | {s['working_set_mb_mean']:.1f} | {s['private_mb_mean']:.1f} | "
                f"{s['threads_mean']:.0f} | {s['ctx_switches_per_s_mean']:.0f} | {s['gpu_pct_mean']:.2f} / {s['gpu_pct_max']:.2f} | "
                f"{s['gpu_dedicated_mb_max']:.1f} |")
    out += ["", "## Audio path health", "",
            "| phase | stream | errors | max callback gap ms | dropouts (mid-signal) | clicks | audible % | level dBFS |",
            "|---|---|---|---|---|---|---|---|"]
    for ph in order:
        for key, val in phases[ph]["artifacts"].items():
            if not key.endswith(".rec") or not val:
                continue
            label = key[:-4]
            a = phases[ph]["artifacts"].get(f"{label}.audio") or {}
            out.append(f"| {ph} | {label} | {val.get('errors')} | {val.get('max_callback_gap_ms')} | "
                       f"{a.get('dropouts', a.get('error'))} | {a.get('clicks')} | {a.get('audible_pct')} | {a.get('rms_dbfs')} |")
    out += ["", "## Tine router counters (delta per phase)", "",
            "| phase | parked | callbacks | underruns | overruns | xruns | max callback us | restarted |",
            "|---|---|---|---|---|---|---|---|"]
    for ph in order:
        t = phases[ph]["artifacts"].get("tine")
        if t:
            out.append(f"| {ph} | {t.get('parked', False)} | {t.get('callbacks')} | {t.get('underruns')} | "
                       f"{t.get('overruns')} | {t.get('xruns', '')} | {t.get('max_callback_us')} | "
                       f"{t.get('router_restarted')} |")
    out += ["", "## Game-load probes", "",
            "| phase | work units/s | frame wake late p50 / p99 / max ms | frames late >2 ms | GPU batches/s |",
            "|---|---|---|---|---|"]
    for ph in order:
        c = phases[ph]["artifacts"].get("cpu_stress") or {}
        gp = phases[ph]["artifacts"].get("gpu_stress") or {}
        if c or gp:
            out.append(f"| {ph} | {c.get('work_units_per_s_mean', '')} | "
                       f"{c.get('late_ms_p50', '')} / {c.get('late_ms_p99', '')} / {c.get('late_ms_max', '')} | "
                       f"{c.get('frames_late_over_2ms', '')} | {gp.get('batches_per_s', '')} |")
    with open(os.path.join(run, "summary.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(out) + "\n")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())

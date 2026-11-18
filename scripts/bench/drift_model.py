"""Model of the drift.rs jitter-buffer fill control (T24 / D-035): undershoot after priming per controller variant.

    python -I drift_model.py
"""
T = 960


def run(prime_factor, iwin=None, secs=90):
    fill = prime_factor * T - 480  # post-render fill right after the priming render
    ef = ei = 0.0
    traj = []
    for n in range(secs * 100):
        fill += 480
        err = (fill - T) / T
        ef += 0.02 * (err - ef)
        if iwin is None or abs(err) < iwin:
            ei = max(-1.0, min(1.0, ei + err * 0.002))
        adj = max(-0.001, min(0.001, ef * 0.001 + ei * 0.0005))
        fill -= 480 * (1 + adj)
        traj.append(fill)
    # steady post-render fill for 1x is T-480 = 480: report worst undershoot below that
    lo = min(traj[100:])
    t_lo = traj.index(lo) / 100
    return round(lo), t_lo, [round(traj[i]) for i in (0, 1000, 2000, 3000, 4000, 6000, 8999)]


for label, pf, iw in [("1x, current", 1, None), ("2x, current", 2, None), ("2x, integ |err|<0.25", 2, 0.25),
                      ("2x, integ |err|<0.1", 2, 0.1), ("2x, integ only err<0 or |err|<0.05", 2, 0.05)]:
    lo, t, tr = run(pf, iw)
    print(f"{label:36s} min post-render fill {lo:4d} at {t:5.1f}s  (steady 480; slack lost {480 - lo:4d})  traj@0,10,20,30,40,60,90s {tr}")


def run_drain(prime_factor, rate=0.005, secs=90, drift_ppm=0):
    fill = prime_factor * T - 480
    ef = ei = 0.0
    draining = prime_factor > 1
    traj = []
    end_drain = None
    for n in range(secs * 100):
        fill += 480 * (1 + drift_ppm * 1e-6)
        if draining and fill <= T:
            draining = False
            ef = ei = 0.0
            end_drain = n / 100
        if draining:
            adj = rate
        else:
            err = (fill - T) / T
            ef += 0.02 * (err - ef)
            ei = max(-1.0, min(1.0, ei + err * 0.002))
            adj = max(-0.001, min(0.001, ef * 0.001 + ei * 0.0005))
        fill -= 480 * (1 + adj)
        traj.append(fill)
    lo = min(traj[100:])
    return round(lo), end_drain, [round(traj[i]) for i in (0, 100, 200, 400, 1000, 3000, 8999)]


for ppm in (0, 100, -100):
    lo, ed, tr = run_drain(2, drift_ppm=ppm)
    print(f"2x + explicit 0.5% drain, drift {ppm:+4d}ppm: min post-render fill {lo} (steady 480), drain done at {ed}s, traj@0,1,2,4,10,30,90s {tr}")

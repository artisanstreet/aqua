import re, statistics, sys, pathlib

out = pathlib.Path(sys.argv[1])
runs = int(sys.argv[2])


def fields(line):
    return {k: v for k, v in re.findall(r"(\w+)=(\S+)", line)}


exited = []


def frame_stats(which, run):
    path = out / f"{which}-{run}-stderr.log"
    if not path.exists():
        return []
    return [fields(line) for line in path.read_text(errors="replace").splitlines()
            if line.startswith("aqua-frame-stats")]


def load(which, run):
    text = (out / f"{which}-{run}.log").read_text(errors="replace")
    marks, lats, wakes, gpu, final = {}, {}, None, {}, {}
    exited.append(any(line.startswith("probe-exited") for line in text.splitlines()))
    for line in text.splitlines():
        f = fields(line)
        if line.startswith("probe-cpu"):
            if f.get("threads") != "0":
                marks[f["name"]] = f
        elif line.startswith("probe-latency"):
            if f.get("samples") != "0":
                lats[f["kind"]] = f
        elif line.startswith("probe-wakes"):
            wakes = f
        elif line.startswith("probe-gpumem"):
            gpu[f["name"]] = f
        elif line.startswith("pid="):
            final = f
    return marks, lats, wakes, gpu, final


def num(f, key):
    try:
        return float(f[key])
    except (KeyError, ValueError, TypeError):
        return None


def med(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


rows = {}
for which in ("aqua", "gpui"):
    cols = {k: [] for k in (
        "idle_cpu_pct", "ctx_per_s", "idle_latency_p99", "scroll_p50", "scroll_p90",
        "scroll_p99", "scroll_max", "scroll_over16", "scroll_over50", "scroll_timeouts",
        "scroll_cpu_ms_per_frame", "private_boot", "private_settle", "working_set_settle",
        "gpu_dedicated_mb", "threads", "frame_cpu_avg", "frame_cpu_max", "failed_runs")}
    for run in range(1, runs + 1):
        try:
            marks, lats, wakes, gpu, final = load(which, run)
        except FileNotFoundError:
            continue
        cols["failed_runs"].append(1.0 if exited[-1] or not marks else 0.0)
        stats = frame_stats(which, run)
        if stats:
            cols["frame_cpu_avg"].append(med([num(f, "cpu_avg_ms") for f in stats]))
            cols["frame_cpu_max"].append(max(num(f, "cpu_max_ms") for f in stats))
        i0, i1 = marks.get("idle0"), marks.get("idle1")
        if i0 and i1:
            wall = (num(i1, "unix_ms") - num(i0, "unix_ms")) / 1000.0
            cols["idle_cpu_pct"].append(100.0 * (num(i1, "cpu_ms") - num(i0, "cpu_ms")) / 1000.0 / wall)
        if wakes:
            cols["ctx_per_s"].append(num(wakes, "context_switches_per_sec_avg"))
        il = lats.get("idle")
        if il:
            cols["idle_latency_p99"].append(num(il, "p99_ms"))
        sl = lats.get("scroll")
        if sl:
            for key, field in (("scroll_p50", "p50_ms"), ("scroll_p90", "p90_ms"), ("scroll_p99", "p99_ms"),
                               ("scroll_max", "max_ms"), ("scroll_over16", "over16_ms"),
                               ("scroll_over50", "over50_ms"), ("scroll_timeouts", "timeouts")):
                cols[key].append(num(sl, field))
        s1, i1m = marks.get("scroll1"), marks.get("idle1")
        if s1 and i1m:
            frames = float(sys.argv[3]) if len(sys.argv) > 3 else 600.0
            cols["scroll_cpu_ms_per_frame"].append((num(s1, "cpu_ms") - num(i1m, "cpu_ms")) / frames)
        if marks.get("boot"):
            cols["private_boot"].append(num(marks["boot"], "private_mb"))
        if marks.get("settle"):
            cols["private_settle"].append(num(marks["settle"], "private_mb"))
            cols["working_set_settle"].append(num(marks["settle"], "working_set_mb"))
            cols["threads"].append(num(marks["settle"], "threads"))
        if gpu.get("settle"):
            cols["gpu_dedicated_mb"].append(num(gpu["settle"], "dedicated_mb"))
    rows[which] = {k: med(v) for k, v in cols.items()}


def fmt(v):
    return "n/a" if v is None else f"{v:.2f}"


labels = [
    ("idle_cpu_pct", "idle CPU (% of one core)"),
    ("ctx_per_s", "idle context switches/s"),
    ("idle_latency_p99", "idle UI-thread latency p99 (ms)"),
    ("scroll_p50", "scroll UI-thread latency p50 (ms)"),
    ("scroll_p90", "scroll UI-thread latency p90 (ms)"),
    ("scroll_p99", "scroll UI-thread latency p99 (ms)"),
    ("scroll_max", "scroll UI-thread latency max (ms)"),
    ("scroll_over16", "scroll samples over 16.7 ms"),
    ("scroll_over50", "scroll samples over 50 ms"),
    ("scroll_timeouts", "scroll probe timeouts"),
    ("scroll_cpu_ms_per_frame", "scroll CPU ms per input"),
    ("private_boot", "private MB after boot"),
    ("private_settle", "private MB after scroll settle"),
    ("working_set_settle", "working set MB after settle"),
    ("gpu_dedicated_mb", "GPU dedicated MB"),
    ("threads", "threads"),
    ("frame_cpu_avg", "frame CPU avg ms (aqua frame stats)"),
    ("frame_cpu_max", "frame CPU worst ms (aqua frame stats)"),
    ("failed_runs", "runs that exited or never ran (median 0/1)"),
]
print(f"{'metric':44} {'aqua':>10} {'gpui':>10}")
for key, label in labels:
    print(f"{label:44} {fmt(rows['aqua'][key]):>10} {fmt(rows['gpui'][key]):>10}")


WINDOWED_PRIVATE_MIB = 142.0
WINDOWED_GPU_MIB = 64.0
verdict = []
aqua = rows["aqua"]
if aqua["private_settle"] is None:
    verdict.append("windowed memory gate: NOT MEASURED (aqua never ran)")
else:
    verdict.append(
        f"windowed private memory gate ({WINDOWED_PRIVATE_MIB:.0f} MiB): "
        f"{'PASS' if aqua['private_settle'] <= WINDOWED_PRIVATE_MIB else 'FAIL'} ({aqua['private_settle']:.1f})")
    if aqua["gpu_dedicated_mb"] is not None:
        verdict.append(
            f"windowed GPU dedicated gate ({WINDOWED_GPU_MIB:.0f} MiB): "
            f"{'PASS' if aqua['gpu_dedicated_mb'] <= WINDOWED_GPU_MIB else 'FAIL'} ({aqua['gpu_dedicated_mb']:.1f})")
    gpui = rows["gpui"]
    for key, label in (("scroll_p99", "scroll p99"), ("scroll_over16", "scroll samples over 16.7 ms"), ("idle_cpu_pct", "idle CPU")):
        if aqua[key] is not None and gpui[key] is not None:
            verdict.append(f"{label}: aqua {'no worse than' if aqua[key] <= gpui[key] else 'WORSE than'} gpui ({aqua[key]:.2f} vs {gpui[key]:.2f})")
print()
print("\n".join(verdict))

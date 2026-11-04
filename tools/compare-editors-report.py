import os
import pathlib
import re
import statistics
import sys

out = pathlib.Path(sys.argv[1])
runs = int(sys.argv[2])
scroll_frames = float(sys.argv[3]) if len(sys.argv) > 3 else 600.0
profile = os.environ.get("AQUA_COMPARE_PROFILE", "release")
TIE_MARGIN = float(os.environ.get("AQUA_COMPARE_TIE_MARGIN", "0.03"))
GATE_PRIVATE = float(os.environ.get("AQUA_GATE_PRIVATE_MIB", "142"))
GATE_GPU = float(os.environ.get("AQUA_GATE_GPU_MIB", "64"))


def fields(line):
    return {k: v for k, v in re.findall(r"(\w+)=(\S+)", line)}


def num(f, key):
    try:
        return float(f[key])
    except (KeyError, ValueError, TypeError):
        return None


def med(values):
    values = [v for v in values if v is not None]
    return statistics.median(values) if values else None


def parse(path):
    marks, lats, wakes, gpu, other = {}, {}, {}, {}, {}
    if not path.exists():
        return None
    for line in path.read_text(errors="replace").splitlines():
        f = fields(line)
        if line.startswith("probe-cpu"):
            if f.get("threads") != "0":
                marks[f["name"]] = f
        elif line.startswith("probe-latency"):
            if f.get("samples") != "0":
                lats[f["name"]] = f
        elif line.startswith("probe-wakes"):
            wakes[f["name"]] = f
        elif line.startswith("probe-gpumem"):
            gpu[f["name"]] = f
        elif line.startswith("probe-startup"):
            other["startup"] = f
        elif line.startswith("probe-open "):
            other["open"] = f
        elif line.startswith("probe-exited"):
            other["exited"] = f
        elif line.startswith("foreground_changed="):
            other["foreground"] = f
    return marks, lats, wakes, gpu, other


def cpu_pct(marks, a, b):
    ma, mb = marks.get(a), marks.get(b)
    if not (ma and mb):
        return None
    wall = (num(mb, "unix_ms") - num(ma, "unix_ms")) / 1000.0
    return 100.0 * (num(mb, "cpu_ms") - num(ma, "cpu_ms")) / 1000.0 / wall if wall > 0 else None


def collect(which):
    cols = {}

    def put(key, value):
        cols.setdefault(key, []).append(value)

    for run in range(1, runs + 1):
        main = parse(out / f"{which}-main-{run}.log")
        if main:
            marks, lats, wakes, gpu, other = main
            st = other.get("startup", {})
            put("startup_first_content_ms", num(st, "first_content_ms"))
            put("startup_window_ms", num(st, "window_ms"))
            put("cold_thread_ms", num(other.get("open", {}), "since_launch_ms"))
            put("fallback_clicks", num(other.get("open", {}), "fallback_click"))
            put("idle_cpu_pct", cpu_pct(marks, "idle0", "idle1"))
            put("idle_ctx_per_s", num(wakes.get("idle", {}), "context_switches_per_sec_avg"))
            sl = lats.get("scroll", {})
            for key, field in (("scroll_p50", "p50_ms"), ("scroll_p90", "p90_ms"), ("scroll_p99", "p99_ms"),
                               ("scroll_max", "max_ms"), ("scroll_over8", "over8_ms"), ("scroll_over16", "over16_ms"),
                               ("scroll_timeouts", "timeouts")):
                put(key, num(sl, field))
            s1, i1 = marks.get("scroll1"), marks.get("idle1")
            if s1 and i1:
                put("scroll_cpu_ms_per_input", (num(s1, "cpu_ms") - num(i1, "cpu_ms")) / scroll_frames)
            ty = lats.get("type", {})
            put("type_p50", num(ty, "p50_ms"))
            put("type_p99", num(ty, "p99_ms"))
            rs = lats.get("resize", {})
            put("resize_p50", num(rs, "p50_ms"))
            put("resize_p99", num(rs, "p99_ms"))
            put("resize_max", num(rs, "max_ms"))
            put("resize_over16", num(rs, "over16_ms"))
            put("private_boot", num(marks.get("boot", {}), "private_mb"))
            put("private_settle", num(marks.get("settle", {}), "private_mb"))
            put("working_set_boot", num(marks.get("boot", {}), "working_set_mb"))
            put("working_set_settle", num(marks.get("settle", {}), "working_set_mb"))
            put("gpu_boot", num(gpu.get("boot", {}), "dedicated_mb"))
            put("gpu_settle", num(gpu.get("settle", {}), "dedicated_mb"))
            put("exited", 1.0 if "exited" in other else 0.0)
            put("focus_stolen", 1.0 if other.get("foreground", {}).get("foreground_stolen_by_app") == "True" else 0.0)
        opened = parse(out / f"{which}-open-{run}.log")
        if opened:
            put("open_thread_ms", num(opened[4].get("open", {}), "since_click_ms"))
        stream = parse(out / f"{which}-stream-{run}.log")
        if stream:
            marks, lats, wakes, gpu, other = stream
            put("stream_cpu_pct", cpu_pct(marks, "stream0", "stream1"))
            sl = lats.get("stream_latency", {})
            put("stream_p50", num(sl, "p50_ms"))
            put("stream_p99", num(sl, "p99_ms"))
            put("stream_over16", num(sl, "over16_ms"))
            put("stream_ctx_per_s", num(wakes.get("stream", {}), "context_switches_per_sec_avg"))
    return {k: med(v) for k, v in cols.items()}


rows = {which: collect(which) for which in ("aqua", "gpui")}

TABLE = [
    ("scroll p50 (ms)", "scroll_p50"),
    ("scroll p90 (ms)", "scroll_p90"),
    ("scroll p99 (ms)", "scroll_p99"),
    ("scroll max (ms)", "scroll_max"),
    ("scroll samples over 8.3 ms", "scroll_over8"),
    ("scroll samples over 16.7 ms", "scroll_over16"),
    ("CPU per scroll input (ms)", "scroll_cpu_ms_per_input"),
    ("keystroke to present p50 (ms)", "type_p50"),
    ("keystroke to present p99 (ms)", "type_p99"),
    ("idle CPU (% of one core)", "idle_cpu_pct"),
    ("idle context switches/s", "idle_ctx_per_s"),
    ("private MiB after startup", "private_boot"),
    ("private MiB after scrolls", "private_settle"),
    ("working set MiB after startup", "working_set_boot"),
    ("working set MiB after scrolls", "working_set_settle"),
    ("GPU dedicated MiB after startup", "gpu_boot"),
    ("GPU dedicated MiB after scrolls", "gpu_settle"),
    ("cold start to first content (ms)", "startup_first_content_ms"),
    ("cold start to thread painted (ms)", "cold_thread_ms"),
    ("open large thread, click to painted (ms)", "open_thread_ms"),
    ("resize p50 (ms)", "resize_p50"),
    ("resize p99 (ms)", "resize_p99"),
    ("resize max (ms)", "resize_max"),
    ("resize samples over 16.7 ms", "resize_over16"),
    ("streaming CPU (% of one core)", "stream_cpu_pct"),
    ("streaming latency p50 (ms)", "stream_p50"),
    ("streaming latency p99 (ms)", "stream_p99"),
    ("streaming samples over 16.7 ms", "stream_over16"),
    ("streaming context switches/s", "stream_ctx_per_s"),
]


def fmt(v):
    return "n/a" if v is None else f"{v:.2f}"


lines = [f"{'metric':44} {'aqua':>10} {'gpui':>10}  verdict"]
verdict = []
failed = False


def beats(a, g):
    if a is None or g is None:
        return None
    if a == 0 and g == 0:
        return True
    return a <= g * (1.0 - TIE_MARGIN)


for label, key in TABLE:
    a, g = rows["aqua"].get(key), rows["gpui"].get(key)
    result = beats(a, g)
    word = {True: "PASS", False: "FAIL", None: "MISSING"}[result]
    lines.append(f"{label:44} {fmt(a):>10} {fmt(g):>10}  {word}")
    verdict.append(f"{label}: {word} (aqua {fmt(a)} vs gpui {fmt(g)}, must beat gpui by {TIE_MARGIN * 100:.0f}% or tie at 0)")
    if result is not True:
        failed = True

for which in ("aqua", "gpui"):
    r = rows[which]
    ok = (r.get("exited") or 0) == 0 and r.get("private_settle") is not None
    verdict.append(f"{which} ran to completion: {'PASS' if ok else 'FAIL'}")
    failed = failed or not ok
    verdict.append(f"{which} never took foreground: {'PASS' if not r.get('focus_stolen') else 'FAIL'}")
    failed = failed or bool(r.get("focus_stolen"))

a = rows["aqua"]
for label, key, limit in (("section 11 windowed private memory", "private_settle", GATE_PRIVATE),
                          ("section 11 windowed GPU dedicated", "gpu_settle", GATE_GPU)):
    value = a.get(key)
    passed = value is not None and value <= limit
    verdict.append(f"{label} ({limit:.0f} MiB, {profile}): {'PASS' if passed else 'FAIL'} ({fmt(value)})")
    failed = failed or not passed

for which in ("aqua", "gpui"):
    clicks = rows[which].get("fallback_clicks")
    if clicks:
        verdict.append(f"WARNING {which}: the launch link did not open the thread in some main runs and the harness clicked the sidebar; cold start to thread painted is not comparable")
verdict.append(f"OVERALL: {'FAIL' if failed else 'PASS'} (fixture-based, {profile}, {runs} runs, median)")
text = "\n".join(lines) + "\n\n" + "\n".join(verdict) + "\n"
print(text)
(out / "scoreboard.txt").write_text(text)
(out / "verdict.txt").write_text("\n".join(verdict) + "\n")
sys.exit(1 if failed else 0)

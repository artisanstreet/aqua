import collections
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
MAX_SYSTEM_LOAD = float(os.environ.get("AQUA_COMPARE_MAX_SYSTEM_LOAD", "25"))
FAST_PRESENT_MS = 8.0
TARGETS = ("aqua", "gpui")


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
    if not path.exists():
        return None
    marks, lats, wakes, gpu, other, threads, loads, displays = {}, {}, {}, {}, {}, collections.defaultdict(list), {}, []
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
        elif line.startswith("probe-threads-done"):
            threads["done:" + f["name"]].append(f)
        elif line.startswith("probe-thread "):
            quoted = re.search(r"thread='([^']*)'", line)
            f["thread"] = quoted.group(1) if quoted else ""
            threads[f["name"]].append(f)
        elif line.startswith("probe-load"):
            loads[f["name"]] = f
        elif line.startswith("probe-display"):
            displays.append(line.strip())
        elif line.startswith("probe-startup"):
            other["startup"] = f
        elif line.startswith("probe-open "):
            other["open"] = f
        elif line.startswith("probe-exited"):
            other["exited"] = f
        elif line.startswith("foreground_changed="):
            other["foreground"] = f
        elif line.startswith("key-latency target="):
            other["keys"] = f
    other["displays"] = displays
    return marks, lats, wakes, gpu, other, threads, loads


def cpu_pct(marks, a, b):
    ma, mb = marks.get(a), marks.get(b)
    if not (ma and mb):
        return None
    wall = (num(mb, "unix_ms") - num(ma, "unix_ms")) / 1000.0
    return 100.0 * (num(mb, "cpu_ms") - num(ma, "cpu_ms")) / 1000.0 / wall if wall > 0 else None


def waking_threads(threads, name):
    return [t for t in threads.get(name, []) if (num(t, "ctx_per_s") or 0) >= 1.0]


def thread_label(t):
    label = t.get("thread") or f"tid{t.get('tid')}"
    return f"{label}@{t.get('module') or '?'}"


def collect(which):
    cols = {}
    notes = {"threads": {}, "display": [], "keys": [], "load": []}

    def put(key, value):
        cols.setdefault(key, []).append(value)

    for run in range(1, runs + 1):
        main = parse(out / f"{which}-main-{run}.log")
        if main:
            marks, lats, wakes, gpu, other, threads, loads = main
            st = other.get("startup", {})
            put("startup_first_content_ms", num(st, "first_content_ms"))
            put("startup_window_ms", num(st, "window_ms"))
            put("cold_thread_ms", num(other.get("open", {}), "since_launch_ms"))
            put("fallback_clicks", num(other.get("open", {}), "fallback_click"))
            put("idle_cpu_pct", cpu_pct(marks, "idle0", "idle1"))
            put("idle_ctx_per_s", num(wakes.get("idle", {}), "context_switches_per_sec_avg"))
            idle_threads = threads.get("idle_threads", [])
            put("idle_waking_threads", float(len(waking_threads(threads, "idle_threads"))) if "done:idle_threads" in threads else None)
            put("idle_top_thread_ctx", max((num(t, "ctx_per_s") or 0 for t in idle_threads), default=0.0) if "done:idle_threads" in threads else None)
            put("idle_resized_cpu_pct", cpu_pct(marks, "rs0", "rs1"))
            resized_ctx = num(wakes.get("after_resize", {}), "context_switches_per_sec_avg")
            if resized_ctx is None and "done:after_resize_threads" in threads:
                resized_ctx = sum(num(t, "ctx_per_s") or 0 for t in threads.get("after_resize_threads", []))
            put("idle_resized_ctx_per_s", resized_ctx)
            resized = threads.get("after_resize_threads", [])
            put("idle_resized_waking_threads", float(len(waking_threads(threads, "after_resize_threads"))) if "done:after_resize_threads" in threads else None)
            sl = lats.get("scroll", {})
            for key, field in (("scroll_p50", "p50_ms"), ("scroll_p90", "p90_ms"), ("scroll_p99", "p99_ms"),
                               ("scroll_max", "max_ms"), ("scroll_over4", "over4_ms"), ("scroll_over8", "over8_ms"),
                               ("scroll_over16", "over16_ms"), ("scroll_timeouts", "timeouts")):
                put(key, num(sl, field))
            s1, i1 = marks.get("scroll1"), marks.get("idle1")
            if s1 and i1:
                put("scroll_cpu_ms_per_input", (num(s1, "cpu_ms") - num(i1, "cpu_ms")) / scroll_frames)
            ty = lats.get("type", {})
            put("pixel_type_p50", num(ty, "p50_ms"))
            put("pixel_type_p99", num(ty, "p99_ms"))
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
            for key in ("boot_load", "end_load"):
                put("system_load", num(loads.get(key, {}), "system_cpu_pct"))
            if run == 1:
                notes["threads"]["idle"] = idle_threads
                notes["threads"]["resized"] = resized
            notes["display"].extend(other["displays"][:1])
            notes["load"].append(med([num(loads.get(k, {}), "system_cpu_pct") for k in ("boot_load", "end_load")]))
        opened = parse(out / f"{which}-open-{run}.log")
        if opened:
            put("open_thread_ms", num(opened[4].get("open", {}), "since_click_ms"))
        stream = parse(out / f"{which}-stream-{run}.log")
        if stream:
            marks, lats, wakes, gpu, other, threads, loads = stream
            put("stream_cpu_pct", cpu_pct(marks, "stream0", "stream1"))
            sl = lats.get("stream_latency", {})
            put("stream_p50", num(sl, "p50_ms"))
            put("stream_p99", num(sl, "p99_ms"))
            put("stream_over4", num(sl, "over4_ms"))
            put("stream_over16", num(sl, "over16_ms"))
            put("stream_ctx_per_s", num(wakes.get("stream", {}), "context_switches_per_sec_avg"))
        keys = parse(out / f"{which}-keys-{run}.log")
        if keys:
            k = keys[4].get("keys", {})
            put("key_p50", num(k, "p50_ms"))
            put("key_p90", num(k, "p90_ms"))
            put("key_p99", num(k, "p99_ms"))
            put("key_max", num(k, "max_ms"))
            put("key_over8", num(k, "over8_3"))
            put("key_over16", num(k, "over16_7"))
            put("key_unmatched", num(k, "unmatched"))
            notes["keys"].append((run, num(k, "p50_ms"), num(k, "p99_ms")))
    return {k: med(v) for k, v in cols.items()}, cols, notes


results = {which: collect(which) for which in TARGETS}
rows = {which: results[which][0] for which in TARGETS}
raw = {which: results[which][1] for which in TARGETS}
notes = {which: results[which][2] for which in TARGETS}

TABLE = [
    ("scroll p50 (ms)", "scroll_p50", True),
    ("scroll p90 (ms)", "scroll_p90", True),
    ("scroll p99 (ms)", "scroll_p99", True),
    ("scroll max (ms)", "scroll_max", True),
    ("scroll frames over 4 ms", "scroll_over4", True),
    ("scroll frames over 8.3 ms", "scroll_over8", True),
    ("scroll frames over 16.7 ms", "scroll_over16", True),
    ("CPU per scroll input (ms)", "scroll_cpu_ms_per_input", True),
    ("keystroke to present p50, ETW (ms)", "key_p50", True),
    ("keystroke to present p90, ETW (ms)", "key_p90", True),
    ("keystroke to present p99, ETW (ms)", "key_p99", True),
    ("keystroke to present max, ETW (ms)", "key_max", True),
    ("keystrokes over 8.3 ms, ETW", "key_over8", True),
    ("keystrokes over 16.7 ms, ETW", "key_over16", True),
    ("keystroke to pixel p50, pixel probe (ms)", "pixel_type_p50", False),
    ("keystroke to pixel p99, pixel probe (ms)", "pixel_type_p99", False),
    ("idle CPU (% of one core)", "idle_cpu_pct", True),
    ("idle context switches/s", "idle_ctx_per_s", True),
    ("idle threads waking at 1/s or more", "idle_waking_threads", True),
    ("idle busiest thread context switches/s", "idle_top_thread_ctx", True),
    ("idle CPU after resize (% of one core)", "idle_resized_cpu_pct", True),
    ("idle context switches/s after resize", "idle_resized_ctx_per_s", True),
    ("idle threads waking after resize", "idle_resized_waking_threads", True),
    ("private MiB after startup", "private_boot", True),
    ("private MiB after 600 scrolls", "private_settle", True),
    ("working set MiB after startup", "working_set_boot", True),
    ("working set MiB after 600 scrolls", "working_set_settle", True),
    ("GPU dedicated MiB after startup", "gpu_boot", True),
    ("GPU dedicated MiB after 600 scrolls", "gpu_settle", True),
    ("cold start to first content (ms)", "startup_first_content_ms", True),
    ("cold start to thread painted (ms)", "cold_thread_ms", True),
    ("open large thread, click to painted (ms)", "open_thread_ms", True),
    ("resize p50 (ms)", "resize_p50", True),
    ("resize p99 (ms)", "resize_p99", True),
    ("resize max (ms)", "resize_max", True),
    ("resize frames over 16.7 ms", "resize_over16", True),
    ("streaming CPU (% of one core)", "stream_cpu_pct", True),
    ("streaming latency p50 (ms)", "stream_p50", True),
    ("streaming latency p99 (ms)", "stream_p99", True),
    ("streaming frames over 4 ms", "stream_over4", True),
    ("streaming frames over 16.7 ms", "stream_over16", True),
    ("streaming context switches/s", "stream_ctx_per_s", True),
]


def fmt(v):
    return "n/a" if v is None else f"{v:.2f}"


def beats(a, g):
    if a is None or g is None:
        return None
    if a == 0 and g == 0:
        return True
    return a <= g * (1.0 - TIE_MARGIN)


lines = [f"{'metric':46} {'aqua':>10} {'gpui':>10}  verdict"]
verdict = []
failed = False

for label, key, gated in TABLE:
    a, g = rows["aqua"].get(key), rows["gpui"].get(key)
    result = beats(a, g)
    word = {True: "PASS", False: "FAIL", None: "MISSING"}[result]
    if not gated:
        word = "info " + word
    lines.append(f"{label:46} {fmt(a):>10} {fmt(g):>10}  {word}")
    verdict.append(f"{label}: {word} (aqua {fmt(a)} vs gpui {fmt(g)}, must beat gpui by {TIE_MARGIN * 100:.0f}% or tie at 0)")
    if gated and result is not True:
        failed = True

for which in TARGETS:
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

for which in TARGETS:
    load = rows[which].get("system_load")
    quiet = load is not None and load <= MAX_SYSTEM_LOAD
    verdict.append(f"{which} runs measured on a quiet machine (system CPU {fmt(load)}% median, limit {MAX_SYSTEM_LOAD:.0f}%): {'PASS' if quiet else 'FAIL'}")
    failed = failed or not quiet

for which in TARGETS:
    clicks = rows[which].get("fallback_clicks")
    if clicks:
        verdict.append(f"WARNING {which}: the launch link did not open the thread in some main runs and the harness clicked the sidebar; cold start to thread painted is not comparable")

states = {}
for which in TARGETS:
    for run, p50, p99 in notes[which]["keys"]:
        states.setdefault(run, {})[which] = "fast" if p50 is not None and p50 < FAST_PRESENT_MS else "slow"
mixed = {which: len({s[which] for s in states.values() if which in s}) > 1 for which in TARGETS}
for which in TARGETS:
    if mixed[which]:
        verdict.append(f"WARNING {which}: Present-return latency swung between fast (<{FAST_PRESENT_MS:.0f} ms) and slow display states across runs; the key rows are a mixture, compare per run below")

details = ["", "display state and system load per run"]
for which in TARGETS:
    for line in notes[which]["display"]:
        details.append(f"  {which}: {line}")
    details.append(f"  {which}: system CPU % per run (boot/end median): {[fmt(v) for v in notes[which]['load']]}")
    for run, p50, p99 in notes[which]["keys"]:
        details.append(f"  {which} run {run}: key present p50 {fmt(p50)} ms p99 {fmt(p99)} ms state {states[run][which]}")
for run in sorted(states):
    pair = states[run]
    if "aqua" in pair and "gpui" in pair:
        details.append(f"  run {run}: aqua {pair['aqua']} vs gpui {pair['gpui']}")

details.append("")
details.append("per-thread idle breakdown (run 1, threads with any context switch or CPU)")
for which in TARGETS:
    for label, key in (("idle", "idle"), ("after resize", "resized")):
        listing = sorted(notes[which]["threads"].get(key, []), key=lambda t: (-(num(t, "cpu_ms_per_s") or 0), -(num(t, "ctx_per_s") or 0)))
        details.append(f"  {which} {label}: {len(listing)} active threads")
        for t in listing[:8]:
            details.append(f"    {thread_label(t):48} ctx/s {fmt(num(t, 'ctx_per_s')):>8} cpu ms/s {fmt(num(t, 'cpu_ms_per_s')):>8} started {t.get('started_ms', '?')} ms")


def rates(path, prefix, group):
    totals = collections.Counter()
    count = 0
    if not path.exists():
        return totals, count
    for line in path.read_text(errors="replace").splitlines():
        if not line.startswith(prefix):
            continue
        match = re.search(group + r"\[([^\]]*)\]", line)
        if not match:
            continue
        count += 1
        for item in match.group(1).split(","):
            if "=" in item:
                label, value = item.rsplit("=", 1)
                try:
                    totals[label.strip()] += float(value)
                except ValueError:
                    pass
    return totals, count


details.append("")
details.append("wake sources, aqua instrumented idle run (AQUA_WAKE_STATS, diagnostic only, not gated)")
wake_log = out / "aqua-wakesrc-1-stderr.log"
if wake_log.exists():
    for group in ("turns", "messages", "posts"):
        totals, count = rates(wake_log, "aqua-wakes:", group)
        top = ", ".join(f"{k}={v:.1f}" for k, v in totals.most_common(6)) or "none"
        details.append(f"  {group} (summed rates over {count} reporting seconds): {top}")
    runs_totals, runs_count = rates(wake_log, "aqua-render-runs:", "")
    render_lines = [l.strip() for l in wake_log.read_text(errors="replace").splitlines() if l.startswith("aqua-render-runs:")]
    details.append(f"  render thread runs: {len(render_lines)} reporting seconds; last: {render_lines[-1] if render_lines else 'none'}")
    parsed = parse(out / "aqua-wakesrc-1.log")
    if parsed:
        marks = parsed[0]
        details.append(f"  idle CPU after settle {fmt(cpu_pct(marks, 'wake0', 'wake1'))}%, after setsize {fmt(cpu_pct(marks, 'wake1', 'wake2'))}%")
else:
    details.append("  not collected")

details.append("")
details.append("frames over 4 ms, aqua spike recorder (AQUA_PROFILE, diagnostic only, not gated)")
spike_log = out / "aqua-spikes-1-stderr.log"
if spike_log.exists():
    spikes = [l for l in spike_log.read_text(errors="replace").splitlines() if l.startswith("aqua-spike")]
    dominant = collections.Counter(re.search(r"dominant=(\S+)", l).group(1) for l in spikes if re.search(r"dominant=(\S+)", l))
    roles = collections.Counter(re.search(r"role=(\S+)", l).group(1) for l in spikes if re.search(r"role=(\S+)", l))
    notes_counter = collections.Counter(n for l in spikes for n in re.findall(r"notes=\[([^\]]*)\]", l)[0].split(";") if n) if spikes else collections.Counter()
    worst = sorted(spikes, key=lambda l: -(num(fields(l), "work_ms") or 0))[:3]
    details.append(f"  {len(spikes)} spike frames over 4 ms in {2 * int(os.environ.get("AQUA_COMPARE_DIAG_SCROLL_FRAMES", "300"))} scroll inputs; by role {dict(roles)}; dominant phase {dict(dominant)}")
    details.append(f"  top notes: {', '.join(f'{k}={v}' for k, v in notes_counter.most_common(6)) or 'none'}")
    for l in worst:
        details.append(f"  worst: {l[:360]}")
else:
    details.append("  not collected")

verdict.append(f"OVERALL: {'FAIL' if failed else 'PASS'} (fixture-based, {profile}, {runs} interleaved runs, median)")
text = "\n".join(lines) + "\n" + "\n".join(details) + "\n\n" + "\n".join(verdict) + "\n"
manifest = out / "manifest.txt"
if manifest.exists():
    text += "\nmanifest\n" + manifest.read_text()
print(text)
(out / "scoreboard.txt").write_text(text)
(out / "verdict.txt").write_text("\n".join(verdict) + "\n")
sys.exit(1 if failed else 0)

import csv
import re
import sys

which, log_path, posts_path, csv_path, stderr_path = sys.argv[1:6]
WARMUP = 5
UNMATCHED_MS = 200.0

pid = None
for line in open(log_path):
    match = re.search(r"probe-keypost .* pid=(\d+)", line)
    if match:
        pid = int(match.group(1))
if pid is None:
    sys.exit("no typelat pid in the drive log")
posts = [int(line) for line in open(posts_path) if line.strip()]

present_stops = []
with open(csv_path, newline="") as handle:
    reader = csv.reader(handle)
    header = [cell.strip() for cell in next(reader)]
    id_col, pid_col, time_col = header.index("Event ID"), header.index("PID"), header.index("Clock-Time")
    for row in reader:
        if len(row) <= time_col:
            continue
        if row[id_col].strip() == "43" and int(row[pid_col].strip(), 16) == pid:
            present_stops.append(int(row[time_col].strip()))
present_stops.sort()

latencies = []
unmatched = 0
index = 0
for post in posts[WARMUP:]:
    while index < len(present_stops) and present_stops[index] < post:
        index += 1
    if index == len(present_stops):
        unmatched += 1
        continue
    delta = (present_stops[index] - post) / 10000.0
    if delta > UNMATCHED_MS:
        unmatched += 1
        continue
    latencies.append(delta)

if not latencies:
    sys.exit(f"no keystroke matched a present (posts={len(posts)} presents={len(present_stops)})")
latencies.sort()


def pct(q):
    return latencies[int(round((len(latencies) - 1) * q))]


print(
    f"key-latency target={which} pid={pid} posts={len(posts) - WARMUP} matched={len(latencies)} unmatched={unmatched} "
    f"p50_ms={pct(0.5):.2f} p90_ms={pct(0.9):.2f} p99_ms={pct(0.99):.2f} max_ms={latencies[-1]:.2f} "
    f"mean_ms={sum(latencies) / len(latencies):.2f} over8_3={sum(v > 8.3 for v in latencies)} over16_7={sum(v > 16.7 for v in latencies)}"
)
try:
    summary = [line for line in open(stderr_path, errors="replace") if "aqua-key-latency-summary" in line]
    for line in summary[-3:]:
        print(line.strip())
except FileNotFoundError:
    pass

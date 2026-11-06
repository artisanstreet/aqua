import json
import random
import sys

SEED = 20261104
TURNS = int(sys.argv[2]) if len(sys.argv) > 2 else 150
STREAM_SECONDS = 24
STREAM_TICK_MS = 33

WORDS = (
    "frame layout renderer scheduler cache atlas glyph shader pipeline buffer queue window input event "
    "runtime signal reactive style motion spring track animation surface swapchain present submit record "
    "transient allocation heap budget gate trim latency scroll viewport damage region occlusion thread "
    "channel transport message command fence barrier descriptor texture sampler sprite quad path stroke "
    "gradient shadow blur clip opacity layer composite tile page index range offset measure wrap shape "
    "cluster fallback font strike outline curve coverage tone linear pixel device adapter driver profile"
).split()

SENTENCES = [
    "The {a} owns the {b} and hands the {c} to the {d} once per frame.",
    "Every {a} is validated before the {b} reaches the {c}, so a stale {d} never survives a resize.",
    "Moving the {a} into the {b} removed the redundant {c} that showed up in the {d} profile.",
    "The {a} grows geometrically, which keeps the {b} amortised while the {c} stays resident.",
    "A cold {a} costs far more than a warm one, so the {b} is prepared before the first {c} arrives.",
    "Because the {a} is immutable, the {b} can be shared between the {c} and the {d} without copying.",
    "The measurement shows the {a} dominates, not the {b}, which is why the {c} was left alone.",
    "When the {a} is occluded the {b} stops producing a {c}, and the {d} wakes it again on exposure.",
]

LANGS = ["rust", "typescript", "python", "json", "sh", "toml"]

RUST = '''pub fn plan_frame(scene: &Scene, budget: &Budget) -> Result<FramePlan, PlanError> {
    let mut plan = FramePlan::with_capacity(scene.len());
    for item in scene.iter() {
        match item.kind() {
            Kind::Quad(quad) => plan.push_quad(quad.bounds, quad.fill),
            Kind::Glyphs(run) => plan.push_glyphs(run.origin, run.ids(), run.color),
            Kind::Image(image) if budget.allows(image.bytes()) => plan.push_image(image.handle()),
            Kind::Image(_) => return Err(PlanError::OverBudget),
        }
    }
    plan.finish()
}
'''

TS = '''export const merge_ranges = (ranges: readonly Range[]): readonly Range[] =>
  pipe(
    ranges,
    Array.sort(Order.mapInput(Order.number, (range: Range) => range.start)),
    Array.reduce([] as Range[], (merged, next) => {
      const last = merged.at(-1)
      return last !== undefined && next.start <= last.end
        ? [...merged.slice(0, -1), { start: last.start, end: Math.max(last.end, next.end) }]
        : [...merged, next]
    }),
  )
'''

PY = '''def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    index = min(len(ordered) - 1, round((len(ordered) - 1) * fraction))
    return ordered[index]


def summarise(samples: list[float]) -> dict[str, float]:
    return {"p50": percentile(samples, 0.5), "p99": percentile(samples, 0.99), "max": max(samples)}
'''

JSON = '''{
  "frame": 1042,
  "phases": { "build": 0.31, "layout": 0.12, "text": 0.08, "lower": 0.05, "record": 0.09 },
  "cache": { "hits": 4812, "misses": 3, "evictions": 0 }
}
'''

SH = '''cargo build --release --target x86_64-pc-windows-gnu -p editor-aqua
./compare-editors.sh run
grep -c "over16" "$OUT/verdict.txt"
'''

TOML = '''[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 16
debug = 1
'''

CODE = {"rust": RUST, "typescript": TS, "python": PY, "json": JSON, "sh": SH, "toml": TOML}


class Gen:
    def __init__(self):
        self.rng = random.Random(SEED)
        self.n = 0
        self.items = []
        self.images = {}

    def pick(self, seq):
        return seq[self.rng.randrange(len(seq))]

    def sentence(self):
        template = self.pick(SENTENCES)
        return template.format(a=self.pick(WORDS), b=self.pick(WORDS), c=self.pick(WORDS), d=self.pick(WORDS))

    def paragraph(self, sentences=None):
        count = sentences or self.rng.randint(2, 5)
        text = " ".join(self.sentence() for _ in range(count))
        if self.rng.random() < 0.35:
            text = text.replace(self.pick(WORDS), f"`{self.pick(WORDS)}`", 1)
        if self.rng.random() < 0.2:
            text += f" See [the {self.pick(WORDS)} notes](https://example.com/{self.pick(WORDS)}/{self.pick(WORDS)})."
        if self.rng.random() < 0.25:
            text = text.replace(self.pick(WORDS), f"**{self.pick(WORDS)}**", 1)
        return text

    def bullets(self):
        marker = self.pick(["-", "*"])
        return "\n".join(f"{marker} {self.sentence()}" for _ in range(self.rng.randint(3, 7)))

    def numbered(self):
        return "\n".join(f"{i + 1}. {self.sentence()}" for i in range(self.rng.randint(3, 6)))

    def table(self):
        rows = self.rng.randint(3, 7)
        lines = ["| phase | p50 (ms) | p99 (ms) | allocations |", "| --- | ---: | ---: | ---: |"]
        for _ in range(rows):
            lines.append(
                f"| {self.pick(WORDS)} | {self.rng.random() * 3:.2f} | {self.rng.random() * 12:.2f} | {self.rng.randint(0, 40)} |"
            )
        return "\n".join(lines)

    def code(self):
        lang = self.pick(LANGS)
        return f"```{lang}\n{CODE[lang]}```"

    def markdown(self, blocks):
        parts = [f"## {self.pick(WORDS).capitalize()} {self.pick(WORDS)}"]
        for _ in range(blocks):
            roll = self.rng.random()
            if roll < 0.40:
                parts.append(self.paragraph())
            elif roll < 0.55:
                parts.append(self.bullets())
            elif roll < 0.65:
                parts.append(self.numbered())
            elif roll < 0.78:
                parts.append(self.code())
            elif roll < 0.86:
                parts.append(self.table())
            elif roll < 0.92:
                parts.append("> " + self.paragraph(2))
            else:
                parts.append(f"### {self.pick(WORDS).capitalize()}\n\n{self.paragraph()}")
        return "\n\n".join(parts)

    def image(self, width=None, height=None):
        name = f"img-{len(self.images)}"
        self.images[name] = {
            "width": width or self.rng.choice([960, 1280, 1600]),
            "height": height or self.rng.choice([540, 720, 900]),
            "seed": len(self.images) + 1,
        }
        return f"blob:{name}"

    def add(self, thread, kind, status="done", run=None, item_id=None):
        self.n += 1
        item = {
            "id": item_id or f"item-{self.n}",
            "thread_id": thread,
            "run_id": run,
            "parent_id": None,
            "native_id": None,
            "status": status,
            "kind": kind,
        }
        self.items.append(item)
        return item

    def tool(self, thread):
        roll = self.rng.random()
        if roll < 0.28:
            command = self.pick(
                ["cargo test -p aqua-layout", "git diff --stat", "rg -n \"frame_budget\" crates", "ls -la target/release", "nix develop --command cargo check"]
            )
            lines = "\n".join(f"{self.pick(WORDS)} {self.pick(WORDS)} ... ok" for _ in range(self.rng.randint(4, 40)))
            tool = {"kind": "shell", "command": command, "cwd": "C:\\bench\\project", "background": False, "watch": False}
            return {"type": "tool", "tool": tool, "title": None, "output": {"text": lines, "omitted_bytes": 0}, "exit_code": 0, "approval": None}
        if roll < 0.52:
            path = f"crates/{self.pick(WORDS)}/src/{self.pick(WORDS)}.rs"
            body = "\n".join(f"{i + 1:>4}  {self.sentence()}" for i in range(self.rng.randint(8, 60)))
            return {"type": "tool", "tool": {"kind": "read", "path": path, "range": [1, 60]}, "title": None, "output": {"text": body, "omitted_bytes": 0}, "exit_code": None, "approval": None}
        if roll < 0.72:
            path = f"crates/{self.pick(WORDS)}/src/{self.pick(WORDS)}.rs"
            diff = "\n".join(
                [f"--- a/{path}", f"+++ b/{path}", "@@ -10,6 +10,9 @@"]
                + [f" {self.sentence()}" for _ in range(2)]
                + [f"-{self.sentence()}" for _ in range(self.rng.randint(1, 4))]
                + [f"+{self.sentence()}" for _ in range(self.rng.randint(2, 6))]
                + [f" {self.sentence()}" for _ in range(2)]
            )
            change = {"path": path, "op": {"op": "modify"}, "diff": diff}
            return {"type": "tool", "tool": {"kind": "edit", "changes": [change]}, "title": None, "output": None, "exit_code": None, "approval": None}
        if roll < 0.84:
            tool = {"kind": "search", "pattern": self.pick(WORDS) + "_" + self.pick(WORDS), "path": None, "mode": "grep"}
            lines = "\n".join(f"crates/{self.pick(WORDS)}/src/{self.pick(WORDS)}.rs:{self.rng.randint(1, 900)}: {self.sentence()}" for _ in range(self.rng.randint(3, 20)))
            return {"type": "tool", "tool": tool, "title": None, "output": {"text": lines, "omitted_bytes": 0}, "exit_code": None, "approval": None}
        if roll < 0.92:
            tool = {"kind": "web_fetch", "url": f"https://docs.example.com/{self.pick(WORDS)}/{self.pick(WORDS)}"}
            return {"type": "tool", "tool": tool, "title": None, "output": {"text": self.paragraph(6), "omitted_bytes": 0}, "exit_code": None, "approval": None}
        tool = {"kind": "mcp", "server": "artisan", "tool": "threads_search", "input": json.dumps({"query": self.pick(WORDS)})}
        return {"type": "tool", "tool": tool, "title": None, "output": {"text": self.paragraph(3), "omitted_bytes": 0}, "exit_code": None, "approval": None}

    def turn(self, thread, index):
        user_images = [self.image()] if self.rng.random() < 0.06 else []
        user_text = self.paragraph(self.rng.randint(1, 3)) if index % 7 else self.markdown(3)
        self.add(thread, {"type": "user_message", "text": user_text, "images": user_images, "steer": False})
        self.add(thread, {"type": "assistant_message", "text": self.paragraph(2), "phase": "commentary"})
        for _ in range(self.rng.randint(2, 8)):
            self.add(thread, self.tool(thread))
            if self.rng.random() < 0.3:
                self.add(thread, {"type": "assistant_message", "text": self.paragraph(1), "phase": "commentary"})
        if self.rng.random() < 0.08:
            self.add(thread, {"type": "image", "blob": self.image(), "caption": self.sentence()})
        if self.rng.random() < 0.05:
            steps = [{"text": self.sentence(), "status": self.pick(["done", "done", "in_progress", "pending"])} for _ in range(self.rng.randint(3, 6))]
            self.add(thread, {"type": "plan", "steps": steps, "proposal": None, "approval": None})
        if self.rng.random() < 0.04:
            self.add(thread, {"type": "notice", "kind": {"type": "info"}, "text": self.sentence()})
        if self.rng.random() < 0.04:
            agent = {"type": "agent", "name": "general-purpose", "description": self.sentence(), "prompt": self.paragraph(), "background": False, "result": self.markdown(3)}
            self.add(thread, agent)
        self.add(thread, {"type": "assistant_message", "text": self.markdown(self.rng.randint(4, 14)), "phase": "final"})

    def stream_steps(self, thread, run):
        steps = []
        item_n = 0
        clock = 0
        words = []
        total_ticks = STREAM_SECONDS * 1000 // STREAM_TICK_MS
        tool_ticks = {int(total_ticks * fraction) for fraction in (0.1, 0.3, 0.5, 0.7)}
        message_id = None
        while len(steps) < total_ticks:
            tick = len(steps)
            if tick in tool_ticks:
                item_n += 1
                steps.append({"at_ms": clock, "item": {
                    "id": f"stream-tool-{item_n}", "thread_id": thread, "run_id": run, "parent_id": None, "native_id": None,
                    "status": "done", "kind": self.tool(thread)}})
                message_id = None
                words = []
            else:
                if message_id is None:
                    item_n += 1
                    message_id = f"stream-msg-{item_n}"
                    words = []
                words.extend(self.pick(WORDS) for _ in range(self.rng.randint(1, 4)))
                if len(words) % 17 == 0:
                    words.append("\n\n")
                steps.append({"at_ms": clock, "item": {
                    "id": message_id, "thread_id": thread, "run_id": run, "parent_id": None, "native_id": None,
                    "status": "running", "kind": {"type": "assistant_message", "text": " ".join(words), "phase": "final"}}})
            clock += STREAM_TICK_MS
        steps.append({"at_ms": clock, "item": {
            "id": message_id or f"stream-msg-{item_n + 1}", "thread_id": thread, "run_id": run, "parent_id": None, "native_id": None,
            "status": "done", "kind": {"type": "assistant_message", "text": " ".join(words) or "done", "phase": "final"}}})
        return steps


VISUAL_PARAGRAPH = (
    "Text sits behind every glass surface in this thread so that blur, tint, shadow and clipping are visible: "
    "the quick brown fox jumps over the lazy dog, then 0123456789 and {{}} [] () <> == != -> => :: ... "
    "with mixed scripts 日本語のテキスト, Ελληνικά, עברית and emoji 🙂 ✨ 👍 😀 🚀 🎉 in the same run."
)


def visual_thread(gen, thread):
    gen.add(thread, {"type": "user_message", "text": "Show emoji 🙂 ✨ 👍 😀 🚀, mixed scripts 日本語 and an image, then a long reply with code and a table.", "images": [], "steer": False})
    gen.add(thread, {"type": "assistant_message", "text": VISUAL_PARAGRAPH, "phase": "commentary"})
    for section in range(3):
        body = [f"## Section {section + 1} 🚀 rendering checks", VISUAL_PARAGRAPH, VISUAL_PARAGRAPH]
        body.append("- bullet one with emoji 👍 and `inline code`\n- bullet two with a [link](https://example.com/visual) and **bold text**\n- bullet three: " + VISUAL_PARAGRAPH[:90])
        body.append("```rust\n" + RUST + "```")
        body.append("| phase | p50 (ms) | p99 (ms) | note |\n| --- | ---: | ---: | --- |\n| build | 0.31 | 1.20 | ✨ warm |\n| layout | 0.12 | 0.50 | 🙂 cached |\n| record | 0.09 | 0.40 | 👍 ok |")
        body.append("> " + VISUAL_PARAGRAPH)
        gen.add(thread, {"type": "assistant_message", "text": "\n\n".join(body), "phase": "final"})
    gen.add(thread, {"type": "image", "blob": gen.image(1280, 720), "caption": "A generated test image with a caption 🙂"})
    gen.add(thread, {"type": "user_message", "text": "Here is the screenshot 🙂", "images": [gen.image(960, 540)], "steer": False})
    gen.add(thread, {"type": "assistant_message", "text": "\n\n".join([VISUAL_PARAGRAPH] * 2), "phase": "final"})


def history_thread(gen, thread):
    gen.add(thread, {"type": "user_message", "text": "An older, finished thread that lives under History.", "images": [], "steer": False})
    gen.add(thread, {"type": "assistant_message", "text": "Settled and filed away.", "phase": "final"})


def main():
    out = sys.argv[1]
    gen = Gen()
    large, stream, project, visual, history = "bench-large", "bench-stream", "bench-project", "bench-visual", "bench-history"
    for index in range(TURNS):
        gen.turn(large, index)
    for index in range(4):
        gen.turn(stream, index)
    gen.add(stream, {"type": "user_message", "text": "Walk me through the frame pipeline and show the hot loop.", "images": [], "steer": False})
    stream_steps = gen.stream_steps(stream, "bench-run")
    visual_thread(gen, visual)
    history_thread(gen, history)
    config = {"engine": "claude", "model": "claude-opus-5-5", "effort": "medium", "permissions": "auto", "options": {}}
    spec = {
        "projects": [{"id": project, "path": "C:\\bench\\project", "name": "Bench"}],
        "threads": [
            {"id": large, "project_id": project, "config": config, "title": "Large benchmark thread", "parent_thread_id": None, "spawned_by": None},
            {"id": stream, "project_id": project, "config": config, "title": "Streaming benchmark thread", "parent_thread_id": None, "spawned_by": None},
            {"id": history, "project_id": project, "config": config, "title": "Finished history thread", "parent_thread_id": None, "spawned_by": None},
            {"id": visual, "project_id": project, "config": config, "title": "Visual parity thread", "parent_thread_id": None, "spawned_by": None},
        ],
        "images": gen.images,
        "items": gen.items,
        "unread": [large, stream, visual],
        "settled": [history],
        "stream": {"thread_id": stream, "run_id": "bench-run", "model": "claude-opus-5-5", "steps": stream_steps},
    }
    with open(out, "w") as file:
        json.dump(spec, file, separators=(",", ":"))
    print(f"items={len(gen.items)} images={len(gen.images)} stream_steps={len(spec['stream']['steps'])} bytes={sum(len(json.dumps(i)) for i in gen.items)}")


main()

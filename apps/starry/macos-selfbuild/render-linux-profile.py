#!/usr/bin/env python3
"""Render one exported guest-linux-profile run; never run tracing on the host."""

import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: render-linux-profile.py <run-directory>")
    run = Path(sys.argv[1]).resolve()
    artifacts = run / "artifacts" / "linux-profile-artifacts"
    maps, messages = read_trace(artifacts / "kernel-profile.raw")
    cpu, active, offcpu = folded_stacks(maps)
    total = sum(cpu.values())
    active_count = sum(active.values())
    if not total:
        raise ValueError("Linux profile has no CPU samples")
    by_cpu = cpu_states(maps["@cpu_state"])
    if sum(row["total"] for row in by_cpu.values()) != total:
        raise ValueError("CPU stack counts disagree with per-CPU sample counts")
    if sum(row["active"] for row in by_cpu.values()) != active_count:
        raise ValueError("idle task names disagree with sampled tid == 0 state")
    summary = {
        "messages": messages,
        "cpu": {
            "total_samples": total,
            "active_samples": active_count,
            "idle_samples": total - active_count,
            "active_percent": 100 * active_count / total,
            "by_cpu": by_cpu,
            "active_leaf_top": top_leaves(active),
        },
        "offcpu": {"estimated_total_ns": sum(offcpu.values()),
                   "sampled_count": int(maps.get("@offcpu_count", 0)),
                   "stack_top": offcpu.most_common(20)},
        "proc_stat": stat_delta(artifacts / "cpu-before.stat", artifacts / "cpu-after.stat"),
        "limitations": [
            "An empty kernel stack is not proof of user mode; it remains explicitly unknown.",
            "off-CPU includes time waiting to run after wakeup and excludes unfinished intervals.",
            "off-CPU is sampled 1/8 and scaled; it is not additive wall time or ext4 mutex time.",
        ],
    }
    output = run / "rendered"
    output.mkdir(exist_ok=True)
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    flamegraph = Path(__file__).resolve().parents[4] / "FlameGraph" / "flamegraph.pl"
    for name, stacks, unit in (("cpu", cpu, "samples"),
                               ("cpu-active", active, "samples"),
                               ("off-cpu", offcpu, "nanoseconds")):
        folded = output / f"{name}.folded"
        folded.write_text("".join(f"{stack} {value}\n" for stack, value in sorted(stacks.items())))
        if stacks and flamegraph.is_file():
            with (output / f"{name}.svg").open("w") as svg:
                subprocess.run([str(flamegraph), "--countname", unit,
                                "--title", f"Linux guest {name}", str(folded)],
                               stdout=svg, check=True)
    if not flamegraph.is_file():
        print(f"flamegraph renderer missing; folded stacks retained: {flamegraph}")
    print(json.dumps({"cpu": summary["cpu"], "proc_stat": summary["proc_stat"]}, indent=2))
    print(f"summary={output / 'summary.json'}")
    return 0


def read_trace(path: Path) -> tuple[dict, list[str]]:
    maps = {}
    messages = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip():
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}:{number}: invalid JSON: {line}") from error
        kind = record["type"]
        if kind in {"errorf", "helper_error", "runtime_error", "lost_events"}:
            raise ValueError(f"kernel tracing reported {kind}: {line}")
        if kind == "map":
            for name, entries in record["data"].items():
                if name in maps:
                    raise ValueError(f"duplicate final map: {name}")
                maps[name] = entries
        elif kind == "printf":
            messages.append(record["data"].strip())
    if not any(line.startswith("LINUX_KERNEL_PROFILE_END ") for line in messages):
        raise ValueError("missing Linux kernel profile END marker")
    return maps, messages


def folded_stacks(maps: dict) -> tuple[Counter, Counter, Counter]:
    cpu, active, offcpu = Counter(), Counter(), Counter()
    for name, destination in (("@cpu", cpu), ("@offcpu", offcpu)):
        for key, value in maps.get(name, {}).items():
            stack, task = key.rsplit(",", 1)
            task = task.strip()
            frames = [frame.strip() for frame in stack.splitlines() if frame.strip()]
            frames = [re.sub(r"\+\d+$", "", frame) for frame in reversed(frames)]
            frames = frames or ["[no-kernel-stack]"]
            frames = [frame.replace(";", ":") for frame in frames]
            folded = ";".join([f"task:{task.replace(';', ':')}", *frames])
            destination[folded] += int(value)
            if name == "@cpu" and not (task == "swapper" or task.startswith("swapper/")):
                active[folded] += int(value)
    return cpu, active, offcpu


def cpu_states(entries: dict) -> dict:
    result = {}
    for key, value in entries.items():
        cpu, idle = (part.strip() for part in key.split(","))
        if idle not in {"0", "1", "false", "true"}:
            raise ValueError(f"invalid idle flag: {key}")
        row = result.setdefault(cpu, {"total": 0, "active": 0, "idle": 0})
        count = int(value)
        row["total"] += count
        row["idle" if idle in {"1", "true"} else "active"] += count
    return result


def top_leaves(stacks: Counter) -> list[dict]:
    leaves = Counter()
    for stack, count in stacks.items():
        leaves[stack.rsplit(";", 1)[-1]] += count
    total = sum(leaves.values())
    return [{"stack": leaf, "value": count, "percent": 100 * count / total}
            for leaf, count in leaves.most_common(20)]


def stat_delta(before: Path, after: Path) -> dict:
    # Linux guest/guest_nice are already included in user/nice: sum only 8 fields.
    def aggregate(path):
        row = next(line.split() for line in path.read_text().splitlines()
                   if line.startswith("cpu "))
        return [int(value) for value in row[1:9]]

    delta = [end - start for start, end in zip(aggregate(before), aggregate(after))]
    if len(delta) != 8 or min(delta) < 0 or sum(delta) <= 0:
        raise ValueError(f"invalid monotonic CPU counters: {delta}")
    names = ("user", "nice", "system", "idle", "iowait", "irq", "softirq", "steal")
    result = dict(zip(names, delta))
    total = sum(delta)
    result["total_ticks"] = total
    result["busy_percent"] = 100 * (total - delta[3] - delta[4] - delta[7]) / total
    return result


if __name__ == "__main__":
    sys.exit(main())

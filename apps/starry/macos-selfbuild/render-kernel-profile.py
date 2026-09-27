#!/usr/bin/env python3
"""Symbolize and summarize a StarryOS in-kernel profile snapshot."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path


EVENT_NAMES = {
    1: "mutex-wait",
    2: "ext4-inclusive",
    3: "page-cache-inclusive",
    4: "block-read-sync",
    5: "block-write-sync",
    6: "off-cpu",
    7: "ext4-lock-wait",
    8: "ext4-lock-hold",
    9: "block-flush-sync",
    # Keep diagnostic snapshots readable even when their hooks are not enabled.
    10: "block-dispatch",
    11: "block-completion",
    12: "block-resume",
    13: "block-write-dispatch",
    14: "block-write-completion",
    15: "block-write-resume",
    16: "block-flush-dispatch",
    17: "block-flush-completion",
    18: "block-flush-resume",
}
CPU_RE = re.compile(
    r"^STARRY_CPU phase=(\d+)(?: cpu=(\d+))? "
    r"samples=(\d+) task=([0-9a-f]*) stack=(.*)$"
)
WAIT_RE = re.compile(
    r"^STARRY_WAIT phase=(\d+)(?: cpu=(\d+))? event=(\d+) count=(\d+) "
    r"total_ns=(\d+) max_ns=(\d+) task=([0-9a-f]*) stack=(.*)$"
)
INSTRUMENTATION_FRAMES = (
    "starry_kernel::profiler::",
    "axsync::profile::",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Summarize a raw /proc/starry_profile snapshot"
    )
    parser.add_argument("run_directory", type=Path, help="saved QEMU experiment directory")
    args = parser.parse_args()
    run = args.run_directory.resolve()
    args.raw = run / "artifacts/arceos-helloworld-profile/kernel-profile.raw"
    args.elf = run / "starryos.elf"
    args.out = run / "rendered"
    args.addr2line = "addr2line"
    workspace = Path(__file__).resolve().parents[3]
    renderer = workspace.parent / "FlameGraph/flamegraph.pl"
    args.flamegraph = renderer if renderer.is_file() else None
    for required in (args.raw, args.elf):
        if not required.is_file():
            parser.error(f"required experiment artifact is missing: {required}")
    if args.flamegraph is None:
        print(f"SVG rendering unavailable: {renderer} is missing; writing folded stacks and JSON")
    return args


def decode_task(raw: str) -> str:
    if not raw:
        return "unknown"
    return bytes.fromhex(raw).decode(errors="replace")


def parse_stack(raw: str) -> tuple[str, ...]:
    return tuple(frame for frame in raw.strip().split(";") if frame)


def read_profile(path: Path) -> tuple[dict[str, int | bool], list[dict], list[dict]]:
    header: dict[str, int | bool] = {}
    cpu_rows: list[dict] = []
    wait_rows: list[dict] = []

    for raw_line in path.read_text(errors="replace").splitlines():
        line = raw_line.replace("\r", "")
        if line.startswith("STARRY_PROFILE_V1 "):
            for key, value in re.findall(r"(\w+)=([^ ]+)", line):
                if value in {"true", "false"}:
                    header[key] = value == "true"
                elif value.isdigit():
                    header[key] = int(value)
            continue
        cpu_match = CPU_RE.match(line)
        if cpu_match:
            cpu_rows.append(
                {
                    "phase": int(cpu_match.group(1)),
                    "cpu": int(cpu_match.group(2)) if cpu_match.group(2) else None,
                    "samples": int(cpu_match.group(3)),
                    "task": decode_task(cpu_match.group(4)),
                    "stack": parse_stack(cpu_match.group(5)),
                }
            )
            continue
        wait_match = WAIT_RE.match(line)
        if wait_match:
            wait_rows.append(
                {
                    "phase": int(wait_match.group(1)),
                    "cpu": int(wait_match.group(2)) if wait_match.group(2) else None,
                    "event": int(wait_match.group(3)),
                    "count": int(wait_match.group(4)),
                    "total_ns": int(wait_match.group(5)),
                    "max_ns": int(wait_match.group(6)),
                    "task": decode_task(wait_match.group(7)),
                    "stack": parse_stack(wait_match.group(8)),
                }
            )

    if not header or (not cpu_rows and not wait_rows):
        raise SystemExit(f"no Starry kernel profile records found in {path}")
    return header, cpu_rows, wait_rows


def symbolize(
    addresses: set[str], elf: Path, addr2line: str
) -> dict[str, str]:
    numeric = sorted(address for address in addresses if address != "user")
    symbols = {"user": "[user-space]"}
    if not numeric:
        return symbols

    process = subprocess.run(
        [addr2line, "-C", "-f", "-e", str(elf)],
        input="\n".join(numeric) + "\n",
        text=True,
        capture_output=True,
        check=True,
    )
    lines = process.stdout.splitlines()
    if len(lines) != len(numeric) * 2:
        raise SystemExit("addr2line returned an unexpected number of lines")
    for index, address in enumerate(numeric):
        function = lines[index * 2].strip()
        location = lines[index * 2 + 1].strip()
        if function in {"", "??"}:
            function = address
        if location not in {"", "??:0", "??:?"}:
            function = f"{function} ({location})"
        symbols[address] = function.replace(";", ":").replace("\n", " ")
    return symbols


def symbolized_stack(stack: tuple[str, ...], symbols: dict[str, str]) -> tuple[str, ...]:
    frames = tuple(symbols.get(address, address) for address in stack)
    return tuple(
        frame
        for frame in frames
        if not any(marker in frame for marker in INSTRUMENTATION_FRAMES)
    )


def folded_stack(root: str, task: str, stack: tuple[str, ...]) -> str:
    safe_task = task.replace(";", ":").replace("\n", " ")
    return ";".join((root, f"task:{safe_task}", *reversed(stack)))


def write_folded(path: Path, values: dict[str, int]) -> None:
    lines = [f"{stack} {value}" for stack, value in sorted(values.items()) if value > 0]
    path.write_text("\n".join(lines) + ("\n" if lines else ""))


def top_rows(values: dict[str, int], limit: int = 30) -> list[dict]:
    total = sum(values.values())
    return [
        {
            "stack": stack,
            "value": value,
            "percent": value * 100.0 / total if total else 0.0,
        }
        for stack, value in sorted(values.items(), key=lambda item: item[1], reverse=True)[
            :limit
        ]
    ]


def render_flamegraph(renderer: Path, folded: Path, output: Path, title: str) -> None:
    count_name = "nanoseconds" if folded.stem in EVENT_NAMES.values() else "samples"
    with folded.open() as source, output.open("w") as destination:
        subprocess.run(
            [
                str(renderer),
                "--countname",
                count_name,
                "--title",
                title,
            ],
            stdin=source,
            stdout=destination,
            check=True,
        )


def main() -> None:
    args = parse_args()
    header, cpu_rows, wait_rows = read_profile(args.raw)
    addresses = {
        address
        for row in [*cpu_rows, *wait_rows]
        for address in row["stack"]
    }
    symbols = symbolize(addresses, args.elf, args.addr2line)
    args.out.mkdir(parents=True, exist_ok=True)

    cpu_values: dict[str, int] = defaultdict(int)
    cpu_active_values: dict[str, int] = defaultdict(int)
    cpu_active_leaves: dict[str, int] = defaultdict(int)
    cpu_by_task: dict[str, int] = defaultdict(int)
    cpu_by_vcpu: dict[int, int] = defaultdict(int)
    cpu_by_vcpu_active: dict[int, int] = defaultdict(int)
    cpu_by_vcpu_idle: dict[int, int] = defaultdict(int)
    cpu_by_vcpu_task: dict[int, dict[str, int]] = defaultdict(
        lambda: defaultdict(int)
    )
    for row in cpu_rows:
        stack = symbolized_stack(row["stack"], symbols)
        key = folded_stack("starry-cpu", row["task"], stack)
        cpu_values[key] += row["samples"]
        cpu_by_task[row["task"]] += row["samples"]
        if row["task"] != "idle":
            cpu_active_values[key] += row["samples"]
            cpu_active_leaves[stack[0] if stack else "[unknown]"] += row["samples"]
        if row["cpu"] is not None:
            cpu = row["cpu"]
            samples = row["samples"]
            cpu_by_vcpu[cpu] += samples
            cpu_by_vcpu_task[cpu][row["task"]] += samples
            cpu_by_vcpu_active[cpu] += 0
            cpu_by_vcpu_idle[cpu] += 0
            if row["task"] == "idle":
                cpu_by_vcpu_idle[cpu] += samples
            else:
                cpu_by_vcpu_active[cpu] += samples
    cpu_folded = args.out / "cpu.folded"
    write_folded(cpu_folded, cpu_values)
    cpu_active_folded = args.out / "cpu-active.folded"
    write_folded(cpu_active_folded, cpu_active_values)
    total_samples = sum(cpu_values.values())
    active_samples = sum(cpu_active_values.values())

    wait_summary = {}
    for event, event_name in EVENT_NAMES.items():
        event_values: dict[str, int] = defaultdict(int)
        event_by_vcpu: dict[int, int] = defaultdict(int)
        event_by_leaf: dict[str, int] = defaultdict(int)
        event_by_caller: dict[str, int] = defaultdict(int)
        page_fault_total_ns = 0
        event_rows = [row for row in wait_rows if row["event"] == event]
        for row in event_rows:
            stack = symbolized_stack(row["stack"], symbols)
            key = folded_stack(f"starry-wait:{event_name}", row["task"], stack)
            event_values[key] += row["total_ns"]
            event_by_leaf[stack[0] if stack else "[unknown]"] += row["total_ns"]
            caller = stack[1] if len(stack) > 1 else "[missing-caller]"
            event_by_caller[caller] += row["total_ns"]
            # This subset overlaps the caller totals; it is not extra latency.
            if any(frame.startswith("starry_kernel::task::user::handle_user_page_fault") for frame in stack):
                page_fault_total_ns += row["total_ns"]
            if row["cpu"] is not None:
                event_by_vcpu[row["cpu"]] += row["total_ns"]
        folded = args.out / f"{event_name}.folded"
        write_folded(folded, event_values)
        wait_summary[event_name] = {
            "sampled_count": sum(row["count"] for row in event_rows),
            "total_ns": sum(row["total_ns"] for row in event_rows),
            "max_ns": max((row["max_ns"] for row in event_rows), default=0),
            "by_cpu_total_ns": dict(sorted(event_by_vcpu.items())),
            "leaf_top": top_rows(event_by_leaf),
            "caller_top": top_rows(event_by_caller),
            "page_fault_total_ns": page_fault_total_ns,
            "top": top_rows(event_values),
        }
        if args.flamegraph and event_values:
            render_flamegraph(
                args.flamegraph,
                folded,
                args.out / f"{event_name}.svg",
                f"StarryOS {event_name}",
            )

    summary = {
        "header": header,
        "cpu": {
            "total_samples": total_samples,
            "active_samples": active_samples,
            "idle_samples": total_samples - active_samples,
            "active_percent": active_samples * 100.0 / total_samples if total_samples else 0.0,
            "active_leaf_top": top_rows(cpu_active_leaves),
            "by_task": dict(sorted(cpu_by_task.items())),
            "by_cpu": dict(sorted(cpu_by_vcpu.items())),
            "by_cpu_active_samples": dict(sorted(cpu_by_vcpu_active.items())),
            "by_cpu_idle_samples": dict(sorted(cpu_by_vcpu_idle.items())),
            "by_cpu_active_percent": {
                cpu: cpu_by_vcpu_active[cpu] * 100.0 / total
                for cpu, total in sorted(cpu_by_vcpu.items())
                if total
            },
            "by_cpu_by_task": {
                cpu: dict(sorted(tasks.items()))
                for cpu, tasks in sorted(cpu_by_vcpu_task.items())
            },
            "top": top_rows(cpu_values),
        },
        "wait": wait_summary,
    }
    (args.out / "summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False) + "\n"
    )
    if args.flamegraph and cpu_values:
        render_flamegraph(
            args.flamegraph,
            cpu_folded,
            args.out / "cpu.svg",
            "StarryOS kernel CPU profile",
        )
    if args.flamegraph and cpu_active_values:
        render_flamegraph(
            args.flamegraph,
            cpu_active_folded,
            args.out / "cpu-active.svg",
            "StarryOS active CPU profile (idle excluded)",
        )


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(f"profile command failed (exit {error.returncode}): {error.cmd}", file=sys.stderr)
        if error.stdout:
            print(error.stdout, end="", file=sys.stderr)
        if error.stderr:
            print(error.stderr, end="", file=sys.stderr)
        raise SystemExit(error.returncode) from error

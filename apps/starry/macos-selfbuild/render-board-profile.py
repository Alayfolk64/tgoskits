#!/usr/bin/env python3
"""Render one StarryOS in-kernel profiling result directory."""

from __future__ import annotations

import json
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path


def fields(line: str) -> tuple[dict[str, str], list[int]]:
    values: dict[str, str] = {}
    stack: list[int] = []
    for word in line.split()[1:]:
        key, separator, value = word.partition("=")
        if separator:
            values[key] = value
    encoded = values.get("stack", "")
    if encoded:
        stack = [int(address, 16) for address in encoded.split(";")]
    return values, stack


def symbolize(elf: Path, addresses: set[int]) -> dict[int, str]:
    ordered = sorted(addresses)
    if not ordered:
        return {}
    process = subprocess.run(
        ["llvm-addr2line", "-C", "-f", "-e", str(elf)],
        input="".join(f"{address:#x}\n" for address in ordered),
        text=True,
        capture_output=True,
        check=True,
    )
    output = process.stdout.splitlines()
    symbols: dict[int, str] = {}
    for index, address in enumerate(ordered):
        function_index = index * 2
        function = output[function_index] if function_index < len(output) else "??"
        symbols[address] = function if function != "??" else f"[unknown {address:#x}]"
    return symbols


def folded_stack(stack: list[int], symbols: dict[int, str]) -> str:
    return ";".join(symbols.get(address, f"[user {address:#x}]") for address in reversed(stack))


def write_folded(path: Path, rows: Counter[str]) -> None:
    with path.open("w", encoding="utf-8") as output:
        for stack, value in rows.most_common():
            output.write(f"{stack} {value}\n")


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: render-board-profile.py RUN_DIRECTORY", file=sys.stderr)
        return 2

    run = Path(sys.argv[1]).resolve()
    raw = run / "kernel-profile.raw"
    elf = run / "starryos.elf"
    if not raw.is_file() or not elf.is_file():
        print(f"missing kernel-profile.raw or starryos.elf in {run}", file=sys.stderr)
        return 2

    event_names: dict[int, str] = {}
    cpu_rows: list[tuple[int, int, bool, list[int]]] = []
    wait_rows: list[tuple[int, int, int, int, list[int]]] = []
    block: dict[str, int] = {}
    block_io: dict[str, int] = {}
    dma_pool: dict[str, int] = {}
    header = ""
    addresses: set[int] = set()

    with raw.open(encoding="utf-8") as profile:
        for line in profile:
            line = line.rstrip("\n")
            if line.startswith("STARRY_PROFILE_V2"):
                header = line
            elif line.startswith("STARRY_EVENT "):
                _, event, name = line.split(maxsplit=2)
                event_names[int(event)] = name
            elif line.startswith("STARRY_BLOCK "):
                block_fields, _ = fields(line)
                block = {key: int(value) for key, value in block_fields.items()}
            elif line.startswith("STARRY_BLOCK_IO "):
                block_io_fields, _ = fields(line)
                block_io = {key: int(value) for key, value in block_io_fields.items()}
            elif line.startswith("STARRY_DMA_POOL "):
                dma_pool_fields, _ = fields(line)
                dma_pool = {key: int(value) for key, value in dma_pool_fields.items()}
            elif line.startswith("STARRY_CPU "):
                values, stack = fields(line)
                samples = int(values["samples"])
                user = values["user"] == "1"
                cpu_rows.append((int(values["cpu"]), samples, user, stack))
                if not user:
                    addresses.update(stack)
            elif line.startswith("STARRY_WAIT "):
                values, stack = fields(line)
                wait_rows.append(
                    (
                        int(values["event"]),
                        int(values["count"]),
                        int(values["total_ns"]),
                        int(values["max_ns"]),
                        stack,
                    )
                )
                addresses.update(stack)

    symbols = symbolize(elf, addresses)
    cpu_folded: Counter[str] = Counter()
    cpu_leaf: Counter[str] = Counter()
    cpu_by_id: Counter[int] = Counter()
    user_samples = 0
    kernel_samples = 0
    idle_samples = 0
    for cpu, samples, user, stack in cpu_rows:
        cpu_by_id[cpu] += samples
        if user:
            user_samples += samples
            cpu_folded["[user]"] += samples
            cpu_leaf["[user]"] += samples
            continue
        kernel_samples += samples
        rendered = folded_stack(stack, symbols) or "[kernel unknown]"
        cpu_folded[rendered] += samples
        leaf = symbols.get(stack[0], "[kernel unknown]") if stack else "[kernel unknown]"
        cpu_leaf[leaf] += samples
        if any(
            "idle" in symbols.get(address, "").lower()
            or "wait_for_interrupt" in symbols.get(address, "").lower()
            for address in stack
        ):
            idle_samples += samples

    wait_folded: dict[str, Counter[str]] = defaultdict(Counter)
    wait_events: dict[str, dict[str, int]] = defaultdict(
        lambda: {"sampled_count": 0, "total_ns": 0, "max_ns": 0}
    )
    wait_leaf: dict[str, Counter[str]] = defaultdict(Counter)
    for event, count, total_ns, max_ns, stack in wait_rows:
        name = event_names.get(event, f"event_{event}")
        rendered = folded_stack(stack, symbols) or "[kernel unknown]"
        wait_folded[name][rendered] += total_ns
        leaf = symbols.get(stack[0], "[kernel unknown]") if stack else "[kernel unknown]"
        wait_leaf[name][leaf] += total_ns
        aggregate = wait_events[name]
        aggregate["sampled_count"] += count
        aggregate["total_ns"] += total_ns
        aggregate["max_ns"] = max(aggregate["max_ns"], max_ns)

    write_folded(run / "cpu.folded", cpu_folded)
    for name, rows in wait_folded.items():
        write_folded(run / f"wait-{name}.folded", rows)

    total_samples = user_samples + kernel_samples
    summary = {
        "header": header,
        "block": block,
        "block_io": block_io,
        "dma_pool": dma_pool,
        "cpu": {
            "total_samples": total_samples,
            "user_samples": user_samples,
            "kernel_samples": kernel_samples,
            "idle_samples": idle_samples,
            "active_samples": total_samples - idle_samples,
            "by_cpu": dict(sorted(cpu_by_id.items())),
            "top_leaf": cpu_leaf.most_common(30),
            "top_stack": cpu_folded.most_common(30),
        },
        "wait": {
            name: {
                **values,
                "top_leaf": wait_leaf[name].most_common(20),
                "top_stack": wait_folded[name].most_common(20),
            }
            for name, values in sorted(
                wait_events.items(), key=lambda item: item[1]["total_ns"], reverse=True
            )
        },
    }
    with (run / "summary.json").open("w", encoding="utf-8") as output:
        json.dump(summary, output, indent=2, sort_keys=False)
        output.write("\n")

    print(header)
    print(f"block={block}")
    print(f"block_io={block_io}")
    print(f"dma_pool={dma_pool}")
    print(
        "cpu "
        f"total={total_samples} user={user_samples} kernel={kernel_samples} "
        f"idle={idle_samples} active={total_samples - idle_samples}"
    )
    print("top CPU leaves:")
    for name, samples in cpu_leaf.most_common(15):
        print(f"  {samples:8d} {name}")
    print("wait events:")
    for name, values in sorted(
        wait_events.items(), key=lambda item: item[1]["total_ns"], reverse=True
    ):
        print(
            f"  {values['total_ns'] / 1_000_000_000:12.3f}s "
            f"max={values['max_ns'] / 1_000_000_000:.3f}s "
            f"samples={values['sampled_count']:8d} {name}"
        )
        for leaf, total_ns in wait_leaf[name].most_common(3):
            print(f"      {total_ns / 1_000_000_000:12.3f}s {leaf}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

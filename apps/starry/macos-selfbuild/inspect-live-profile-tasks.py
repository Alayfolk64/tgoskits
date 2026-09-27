#!/usr/bin/env python3
"""Read task state from the frozen full-build QEMU without stopping its CPUs.

Usage: sudo python3 inspect-live-profile-tasks.py RUN_DIRECTORY

This is deliberately tied to one ELF layout, verified from its disassembly.
It neither attaches a debugger nor writes guest memory. Reads are not a global
atomic snapshot: changed task records are reported and must not be interpreted.
"""

import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import time


ELF_SHA256 = "e1a4d024f0b8e1389878aa2b663c2cdac191ac6cc2791be038a02eb7f7cf9424"
BIN_SHA256 = "b2449148f0f1178bc2b7e7bbe90c4019dbc919c36bbc5d50f9f6b2e818e25523"
RAM_SIZE = 8 * 1024**3
PHYSICAL_BASE = 0x40000000
DIRECT_BASE = 0xFFFF000000000000
CPU_AREA_BASE = 0xFFFFFF0000000000
KERNEL_BASE = 0xFFFFFFFF80000000


def main():
    if len(sys.argv) != 2:
        raise ValueError("usage: inspect-live-profile-tasks.py RUN_DIRECTORY")
    run = Path(sys.argv[1]).resolve()
    elf = run / "starryos.elf"
    with elf.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != ELF_SHA256:
        raise ValueError(f"unsupported ELF layout: {digest}")
    with (run / "starryos.bin").open("rb") as source:
        binary_digest = hashlib.file_digest(source, "sha256").hexdigest()
    if binary_digest != BIN_SHA256:
        raise ValueError(f"kernel BIN does not match frozen ELF: {binary_digest}")
    pid = int((run / "qemu.pid").read_text().strip())
    proc = Path("/proc") / str(pid)
    command = (proc / "cmdline").read_bytes().decode().split("\0")
    if "qemu-system-aarch64" not in Path(command[0]).name:
        raise ValueError(f"PID {pid} is not the AArch64 QEMU process")
    if "-kernel" not in command or command[command.index("-kernel") + 1] != str(run / "starryos.bin"):
        raise ValueError("QEMU kernel does not belong to the selected run")
    mappings = []
    for line in (proc / "maps").read_text().splitlines():
        fields = line.split()
        start, end = (int(value, 16) for value in fields[0].split("-"))
        if end - start == RAM_SIZE and fields[1] == "rw-p" and len(fields) == 5:
            mappings.append(start)
    if len(mappings) != 1:
        raise ValueError(f"expected one 8 GiB anonymous RAM mapping, found {mappings}")
    with (proc / "mem").open("rb", buffering=0) as memory:
        reader = GuestMemory(memory.fileno(), mappings[0])
        tasks = read_task_registry(reader)
        queues = read_run_queues(reader)
        profile_summary = save_live_profile(reader, run)
    addresses = sorted({frame for task in tasks for frame in task.get("frames", [])})
    symbols = symbolize(elf, addresses)
    for task in tasks:
        task["stack"] = [symbols[frame] for frame in task.pop("frames", [])]
    report = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "pid": pid,
        "elf_sha256": digest,
        "read_only": True,
        "globally_atomic": False,
        "run_queues": queues,
        "live_profile": profile_summary,
        "tasks": tasks,
    }
    output = run / f"live-tasks-{time.time_ns()}.json"
    with output.open("x") as destination:
        json.dump(report, destination, indent=2)
        destination.write("\n")
    print(json.dumps({"timestamp": report["timestamp"], "run_queues": queues,
                      "task_count": len(tasks), "ready_tasks": [
                          task for task in tasks if task.get("state") == "Ready"
                      ]}, indent=2))
    print(f"Saved read-only snapshot: {output}")
    print(f"Saved unfinished live profile: {profile_summary['directory']}")


class GuestMemory:
    def __init__(self, fd, host_base):
        self.fd = fd
        self.host_base = host_base

    def read(self, address, size):
        if KERNEL_BASE <= address < KERNEL_BASE + 0x1000000:
            physical = 0x40200000 + address - KERNEL_BASE
        elif CPU_AREA_BASE + PHYSICAL_BASE <= address < CPU_AREA_BASE + PHYSICAL_BASE + RAM_SIZE:
            # someboot aarch64::Arch::cpu_area_phys_to_virt uses this alias.
            physical = address - CPU_AREA_BASE
        else:
            physical = address - DIRECT_BASE
        offset = physical - PHYSICAL_BASE
        if not 0 <= offset or not 0 <= size <= RAM_SIZE - offset:
            raise ValueError(f"address outside guest RAM: {address:#x}, size {size}")
        result = os.pread(self.fd, size, self.host_base + offset)
        if len(result) != size:
            raise OSError(f"short guest read at {address:#x}: {len(result)}/{size}")
        return result


def read_task_registry(reader):
    # task_by_id: lock/root/height/length/LazyInit; BTree leaf keys +8,
    # weak Arc pointers +96, len +186, internal edges +192.
    registry = reader.read(0xFFFFFFFF805D0CD0, 40)
    lock, node, height, count, initialized = struct.unpack("<5Q", registry)
    if lock or initialized != 2 or height > 7 or count > 100000:
        raise ValueError("task registry is mutating or its layout is invalid; retry")
    tasks = []
    visited = set()

    def visit(pointer, level):
        if pointer in visited or len(visited) > count + 1:
            raise ValueError("task registry traversal changed; retry")
        visited.add(pointer)
        node = reader.read(pointer, 288 if level else 192)
        length = struct.unpack_from("<H", node, 186)[0]
        if length > 11:
            raise ValueError("invalid BTree node length; retry")
        for index in range(length):
            task_id = struct.unpack_from("<Q", node, 8 + index * 8)[0]
            arc = struct.unpack_from("<Q", node, 96 + index * 8)[0]
            task = read_task(reader, task_id, arc)
            if task is not None:
                tasks.append(task)
        if level:
            for index in range(length + 1):
                visit(struct.unpack_from("<Q", node, 192 + index * 8)[0], level - 1)

    visit(node, height)
    return sorted(tasks, key=lambda task: task["id"])


def read_task(reader, task_id, arc):
    raw = reader.read(arc, 1184)
    if struct.unpack_from("<Q", raw)[0] == 0:
        return None
    if struct.unpack_from("<Q", raw, 1008)[0] != task_id:
        return {"id": task_id, "unstable": True}
    name_ptr, name_len = struct.unpack_from("<2Q", raw, 272)
    if name_len > 256 or raw[256]:
        return {"id": task_id, "unstable": True}
    name = reader.read(name_ptr, name_len).decode("utf-8", "replace") if name_len else ""
    state = raw[1138]
    task = {
        "id": task_id,
        "name": name,
        "arc": hex(arc),
        "state": {1: "Running", 2: "Ready", 3: "Blocked", 4: "Exited"}.get(state, "invalid"),
        "on_cpu": raw[1140],
        "cpu": struct.unpack_from("<I", raw, 1128)[0],
        "wake_handoff": hex(struct.unpack_from("<Q", raw, 1016)[0]),
        "queue_links": [hex(link) for link in struct.unpack_from("<2Q", raw, 1160)],
        "queue_linked": raw[1176],
    }
    if state in (2, 3) and not task["on_cpu"]:
        task["frames"] = read_saved_stack(reader, raw)
    after = reader.read(arc, 1184)
    # No coherence claim beyond the actual fields used for this task record.
    observed_ranges = [(0, 8), (256, 288), (288, 304), (352, 456), (1008, 1024), (1128, 1184)]
    task["unstable"] = any(raw[start:end] != after[start:end] for start, end in observed_ranges)
    return task


def read_saved_stack(reader, raw):
    bottom, size = struct.unpack_from("<2Q", raw, 288)
    sp = struct.unpack_from("<Q", raw, 352)[0]
    fp, lr = struct.unpack_from("<2Q", raw, 440)
    if size != 0x40000 or not bottom <= sp < bottom + size:
        raise ValueError("saved task stack bounds do not match frozen kernel")
    frames = [lr - 4]
    while sp <= fp and fp + 16 <= bottom + size and len(frames) < 32:
        previous, lr = struct.unpack("<2Q", reader.read(fp, 16))
        if not KERNEL_BASE <= lr < KERNEL_BASE + 0x1000000:
            break
        frames.append(lr - 4)
        if previous <= fp or previous % 16:
            break
        fp = previous
    return frames


def read_run_queues(reader):
    pointers = struct.unpack("<8Q", reader.read(0xFFFFFFFF80E3A4E8, 64))
    queues = []
    for cpu, pointer in enumerate(pointers):
        before = reader.read(pointer, 32)
        identity, lock, head, length = struct.unpack("<4Q", before)
        after = reader.read(pointer, 32)
        queues.append({"cpu": cpu, "identity": identity, "lock": lock,
                       "head": hex(head), "length": length, "unstable": before != after})
    return queues


def save_live_profile(reader, run):
    """Preserve aggregate and pending tables, explicitly not a completed run."""
    capacity, pointer, length, initialized = struct.unpack(
        "<4Q", reader.read(0xFFFFFFFF80E39538, 32))
    if (capacity, length, initialized) != (8, 8, 2):
        raise ValueError("unexpected frozen profiler layout")
    profiles = reader.read(pointer, 160 * 8)
    directory = run / f"live-profile-{time.time_ns()}"
    directory.mkdir()
    with (directory / "cpu-profiles.bin").open("xb") as output:
        output.write(profiles)
    rows = []
    changed_rows = 0
    drops = [0, 0, 0]
    clocks = []
    for cpu in range(8):
        profile = profiles[cpu * 160:(cpu + 1) * 160]
        clocks.append(struct.unpack_from("<Q", profile)[0])
        for index, value in enumerate(struct.unpack_from("<3Q", profile, 136)):
            drops[index] += value
        for name, offset, count, stride in [("cpu", 80, 4096, 224),
                                           ("wait", 96, 8192, 224),
                                           ("pending", 112, 512, 216)]:
            table, actual_count = struct.unpack_from("<2Q", profile, offset)
            if actual_count != count:
                raise ValueError(f"unexpected {name} table length: {actual_count}")
            before = reader.read(table, count * stride)
            after = reader.read(table, count * stride)
            with (directory / f"cpu{cpu}-{name}.bin").open("xb") as output:
                output.write(after)
            if name == "pending":
                continue
            for index in range(count):
                start = index * stride
                entry = after[start:start + stride]
                if entry != before[start:start + stride]:
                    changed_rows += 1
                    continue
                if struct.unpack_from("<Q", entry)[0] == 0:
                    continue
                rows.append(format_aggregate(cpu, name, entry))
    phase = reader.read(0xFFFFFFFF80E394C8, 1)[0]
    enabled = bool(reader.read(0xFFFFFFFF80E394CC, 1)[0])
    skipped = struct.unpack("<Q", reader.read(0xFFFFFFFF80E394D0, 8))[0]
    started = struct.unpack("<Q", reader.read(0xFFFFFFFF80E394D8, 8))[0]
    total = struct.unpack("<Q", reader.read(0xFFFFFFFF80E394F0, 8))[0]
    header = ("STARRY_PROFILE_V1 sample_hz=10 ext4_sample_rate=16 "
              "page_cache_sample_rate=32 offcpu_sample_rate=8 ext4_hold_sample_rate=1 "
              f"block_flush_sample_rate=1 enabled={str(enabled).lower()} phase={phase} "
              f"prebuild_ns={total} dropped_cpu={drops[0]} dropped_wait={drops[1]} "
              f"dropped_pending={drops[2]} skipped_cpu={skipped}\n")
    with (directory / "kernel-profile.raw").open("x") as output:
        output.write(header)
        output.write("STARRY_PHASE 1 prebuild\n")
        output.write("\n".join(rows) + "\n")
    summary = {"directory": str(directory), "completed_build": False,
               "globally_atomic": False, "enabled": enabled, "phase": phase,
               "phase_started_ns": started, "last_sample_ns_by_cpu": clocks,
               "host_changed_aggregate_rows_excluded": changed_rows,
               "dropped_cpu_wait_pending": drops, "skipped_cpu": skipped}
    with (directory / "snapshot.json").open("x") as output:
        json.dump(summary, output, indent=2)
        output.write("\n")
    return summary


def format_aggregate(cpu, name, entry):
    # add_aggregate disassembly: hash +0, stack +8, total/count/max +192,
    # phase/event +216. The full 224-byte record is retained above.
    name_len, stack_len = entry[184], entry[185]
    if name_len > 16 or not 0 < stack_len <= 20:
        raise ValueError("invalid frozen aggregate stack/name lengths")
    task = entry[168:168 + name_len].hex()
    frames = struct.unpack_from(f"<{stack_len}Q", entry, 8)
    stack = ";".join("user" if frame == 2**64 - 1 else hex(frame) for frame in frames)
    total, count, maximum = struct.unpack_from("<3Q", entry, 192)
    if name == "cpu":
        return f"STARRY_CPU phase={entry[216]} cpu={cpu} samples={total} task={task} stack={stack}"
    return (f"STARRY_WAIT phase={entry[216]} cpu={cpu} event={entry[217]} "
            f"count={count} total_ns={total} max_ns={maximum} task={task} stack={stack}")


def symbolize(elf, addresses):
    if not addresses:
        return {}
    result = subprocess.run(["addr2line", "-Cf", "-e", str(elf)],
                            input="\n".join(hex(address) for address in addresses),
                            text=True, stdout=subprocess.PIPE, check=True)
    lines = result.stdout.splitlines()
    if len(lines) != len(addresses) * 2:
        raise ValueError("unexpected addr2line output length")
    return {address: {"address": hex(address), "function": lines[index * 2],
                      "location": lines[index * 2 + 1]} for index, address in enumerate(addresses)}


if __name__ == "__main__":
    main()

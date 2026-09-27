#!/usr/bin/env python3
"""Run a saved profiling kernel with the existing persistent QEMU configuration.

Usage: python3 apps/starry/macos-selfbuild/run-profile-qemu.py <run-directory>
The directory must contain starryos.bin and its matching starryos.elf.
Prepare the guest runner and rootfs before invoking this saved-kernel A/B helper.
"""

import os
import subprocess
import sys
import tomllib
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: run-profile-qemu.py <run-directory>")
    run = Path(sys.argv[1]).resolve()
    for name in ("starryos.bin", "starryos.elf"):
        if not (run / name).is_file():
            raise SystemExit(f"saved kernel is missing: {run / name}")
    log = run / "qemu.log"
    if log.exists():
        raise SystemExit(f"refusing to overwrite existing experiment: {log}")
    debug = os.environ.get("STARRY_PROFILE_DEBUG", "0")
    if debug not in {"0", "1"}:
        raise SystemExit("STARRY_PROFILE_DEBUG must be 0 or 1")

    app = Path(__file__).resolve().parent
    workspace = app.parents[2]
    with (app / "qemu-aarch64-profile.toml").open("rb") as source:
        config = tomllib.load(source)
    args = [arg.replace("${workspace}", str(workspace)) for arg in config["args"]]
    command = [
        "qemu-system-aarch64", *args,
        "-device", "virtio-net-pci,netdev=net0",
        "-netdev", "user,id=net0",
        "-kernel", str(run / "starryos.bin"),
        "-pidfile", str(run / "qemu.pid"),
        "-chardev", f"stdio,id=console,logfile={log}",
        "-serial", "chardev:console",
        "-monitor", "none",
    ]
    if debug == "1":
        command.extend([
            "-chardev", "socket,path=gdb.sock,server=on,wait=off,id=debug0",
            "-gdb", "chardev:debug0",
        ])
        print(f"debug_socket={run / 'gdb.sock'}", flush=True)
    print(f"saved_kernel={run / 'starryos.bin'}", flush=True)
    print(f"serial_log={log}", flush=True)
    return subprocess.run(command, cwd=run, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Boot a prepared Linux comparison disk with the Starry profiling hardware.

Usage: python3 apps/starry/macos-selfbuild/run-linux-profile-qemu.py <run-directory>
Prepare vmlinuz, initramfs.img and an independent writable rootfs.img first.
This launcher does not prepare the disk or start the compilation workload.
"""

import subprocess
import sys
import tomllib
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: run-linux-profile-qemu.py <run-directory>")
    run = Path(sys.argv[1]).resolve()
    for name in ("vmlinuz", "initramfs.img", "rootfs.img"):
        path = run / name
        if not path.is_file():
            raise SystemExit(f"prepared Linux artifact is missing: {path}")
    rootfs = run / "rootfs.img"
    if rootfs.is_symlink() or rootfs.stat().st_nlink != 1:
        raise SystemExit(f"Linux rootfs must be an independent copy: {rootfs}")
    if not rootfs.stat().st_mode & 0o200:
        raise SystemExit(f"Linux rootfs is not owner-writable: {rootfs}")
    log = run / "qemu.log"
    if log.exists():
        raise SystemExit(f"refusing to overwrite existing experiment: {log}")

    app = Path(__file__).resolve().parent
    workspace = app.parents[2]
    with (app / "qemu-aarch64-profile.toml").open("rb") as source:
        config = tomllib.load(source)
    args = [arg.replace("${workspace}", str(workspace)) for arg in config["args"]]
    if args.count("-drive") != 1:
        raise SystemExit("comparison requires exactly one configured root drive")
    drive_index = args.index("-drive") + 1
    starry_disk = workspace / "tmp/axbuild/rootfs/rootfs-aarch64-arceos-helloworld-profile.img"
    expected_drive = f"id=disk0,if=none,format=raw,file={starry_disk}"
    if args[drive_index] != expected_drive:
        raise SystemExit(f"unexpected root drive configuration: {args[drive_index]}")
    args[drive_index] = f"id=disk0,if=none,format=raw,file={rootfs}"
    command = [
        "qemu-system-aarch64", *args,
        "-device", "virtio-net-pci,netdev=net0",
        "-netdev", "user,id=net0",
        "-kernel", str(run / "vmlinuz"),
        "-initrd", str(run / "initramfs.img"),
        "-append", "console=ttyAMA0 root=/dev/nvme0n1 rootfstype=ext4 "
        "rootflags=rw modules=nvme,ext4 init=/bin/sh",
        "-pidfile", str(run / "qemu.pid"),
        "-chardev", f"stdio,id=console,logfile={log}",
        "-serial", "chardev:console",
        "-monitor", "none",
    ]
    print(f"linux_kernel={run / 'vmlinuz'}", flush=True)
    print(f"linux_rootfs={rootfs}", flush=True)
    print(f"serial_log={log}", flush=True)
    return subprocess.run(command, cwd=run, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Add matching NVMe/ext4 modules to the pinned Alpine comparison initramfs.

Usage: python3 apps/starry/macos-selfbuild/prepare-linux-profile-initramfs.py
Input downloads and generated files stay in tmp/linux-qemu-profile.
The upstream init script remains unchanged. No disk is mounted or modified.
"""

import gzip
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

VERSION = "6.18.35-0-virt"
DOWNLOADS = {
    "initramfs-virt": "d137f3f7cc813a4c027a0bb4a2826f9fadd3f048b97bc9a0a395300e8e17ef1a",
    "modloop-virt": "2c9155b42b124108fa22f6099ac010e81e20c212c945955c3a5ac24a3bb7f4f3",
}


def main() -> None:
    if len(sys.argv) != 1:
        raise SystemExit("usage: prepare-linux-profile-initramfs.py")
    workspace = Path(__file__).resolve().parents[3]
    cache = workspace / "tmp/linux-qemu-profile"
    output = cache / "initramfs-nvme.img"
    if output.exists():
        raise SystemExit(f"refusing to overwrite prepared initramfs: {output}")
    for name, expected in DOWNLOADS.items():
        path = cache / name
        with path.open("rb") as source:
            actual = hashlib.file_digest(source, "sha256").hexdigest()
        if actual != expected:
            raise SystemExit(f"download checksum mismatch: {path}: {actual} != {expected}")
    staging = Path(tempfile.mkdtemp(prefix="initramfs-", dir=cache))
    print(f"staging={staging}", flush=True)
    modules = staging / "modloop"
    subprocess.run(["unsquashfs", "-d", str(modules), str(cache / "modloop-virt")], check=True)
    build_initramfs(cache / "initramfs-virt", modules / "modules" / VERSION, staging)
    (staging / "initramfs.img").rename(output)
    print(f"prepared_initramfs={output}", flush=True)
    subprocess.run(["sha256sum", str(output)], check=True)


def build_initramfs(original: Path, modules: Path, staging: Path) -> None:
    """Keep the upstream boot files and replace the matching module closure."""
    if not (modules / "modules.dep").is_file():
        raise ValueError(f"module dependency index is missing: {modules}")
    for driver in ("kernel/drivers/nvme/host/nvme.ko", "kernel/fs/ext4/ext4.ko"):
        if not (modules / driver).is_file():
            raise ValueError(f"required root driver is missing: {modules / driver}")
    root = staging / "root"
    root.mkdir()
    subprocess.run(
        ["cpio", "-id", "--no-preserve-owner"], cwd=root,
        input=gzip.decompress(original.read_bytes()), check=True,
    )
    shutil.copytree(modules, root / "lib/modules" / VERSION, dirs_exist_ok=True, symlinks=True)
    # cpio reads a NUL-delimited file list directly; no shell or pipeline is used.
    names = [".", *(str(path.relative_to(root)) for path in sorted(root.rglob("*")))]
    archive = subprocess.run(
        ["cpio", "-o", "-H", "newc", "--null", "--owner=0:0"], cwd=root,
        input="\0".join(names).encode() + b"\0", stdout=subprocess.PIPE, check=True,
    ).stdout
    with (staging / "initramfs.img").open("xb") as destination:
        destination.write(gzip.compress(archive, mtime=0))


if __name__ == "__main__":
    main()

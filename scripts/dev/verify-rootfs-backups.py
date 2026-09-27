#!/usr/bin/env python3
"""Compare the two archived build images with their existing compressed backups."""

from compression import zstd
import hashlib
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
IMAGES = (
    "rootfs-profile-host-config-baseline-window.img",
    "rootfs-profile-inode-writeback-read-cache-window.img",
)


def identity(path):
    stat = path.stat()
    return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns


def verify(image):
    backup = image.with_suffix(image.suffix + ".zst")
    if image.is_symlink() or backup.is_symlink():
        raise RuntimeError(f"Refusing a symbolic link: {image}, {backup}")
    before = identity(image), identity(backup)
    digest = hashlib.sha256()
    compared = 0
    print(f"Comparing {image} with {backup}", flush=True)
    with image.open("rb") as original, zstd.ZstdFile(backup, "rb") as restored:
        while True:
            expected = original.read(1024 * 1024)
            actual = restored.read(1024 * 1024)
            if expected != actual:
                raise RuntimeError(f"Backup differs at chunk offset {compared}: {image}")
            if not expected:
                break
            digest.update(expected)
            compared += len(expected)
    if before != (identity(image), identity(backup)):
        raise RuntimeError(f"Image or backup changed during comparison: {image}")
    print(f"MATCH bytes={compared} sha256={digest.hexdigest()}", flush=True)


def main():
    for name in IMAGES:
        verify(ROOT / "tmp/axbuild/rootfs" / name)


if __name__ == "__main__":
    main()

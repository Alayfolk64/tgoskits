#!/usr/bin/env python3
"""Apply and restore one benchmark-only derive-path compatibility patch."""

import hashlib
import json
import os
import stat
import sys
import tempfile
from pathlib import Path


ROOTFS = Path(os.environ.get("SELFBUILD_ROOTFS", "/opt/starry-orangepi5plus-selfbuild/rootfs"))
CRATE = ROOTFS / "root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/secret-service-5.2.0"
BACKUP = ROOTFS / "output/bench-secret-service-backup"
FILES = {
    "src/proxy/collection.rs": 1,
    "src/proxy/service.rs": 4,
    "src/proxy/mod.rs": 1,
}
DERIVE = b"#[derive(Debug, Serialize, Deserialize, Type)]\n"
EXPLICIT_PATH = b'#[zvariant(crate = "zbus::zvariant")]\n'


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def patched(original: bytes, count: int, relative: str) -> bytes:
    if original.count(DERIVE) != count or EXPLICIT_PATH in original:
        raise RuntimeError(f"unexpected original crate source: {relative}")
    return original.replace(DERIVE, DERIVE + EXPLICIT_PATH)


def replace_file(path: Path, content: bytes) -> None:
    mode = stat.S_IMODE(path.stat().st_mode)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}.", delete=False) as output:
            temporary = Path(output.name)
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        temporary.chmod(mode)
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def saved_sources() -> list[tuple[str, bytes, bytes]]:
    manifest = json.loads((BACKUP / "manifest.json").read_text())
    if {entry["file"] for entry in manifest} != set(FILES) or len(manifest) != len(FILES):
        raise RuntimeError("backup manifest does not match expected files")
    entries = {entry["file"]: entry for entry in manifest}
    records = []
    for relative, count in FILES.items():
        original = (BACKUP / relative).read_bytes()
        replacement = patched(original, count, relative)
        if digest(original) != entries[relative]["original_sha256"]:
            raise RuntimeError(f"backup checksum mismatch: {relative}")
        if digest(replacement) != entries[relative]["patched_sha256"]:
            raise RuntimeError(f"patched checksum mismatch: {relative}")
        current = (CRATE / relative).read_bytes()
        if current not in (original, replacement):
            raise RuntimeError(f"source changed outside benchmark patch: {relative}")
        records.append((relative, original, replacement))
    return records


def apply() -> None:
    if BACKUP.exists():
        records = saved_sources()
    else:
        records = []
        for relative, count in FILES.items():
            original = (CRATE / relative).read_bytes()
            records.append((relative, original, patched(original, count, relative)))
        BACKUP.mkdir(parents=True)
        manifest = []
        for relative, original, replacement in records:
            saved = BACKUP / relative
            saved.parent.mkdir(parents=True, exist_ok=True)
            saved.write_bytes(original)
            manifest.append({
                "file": relative,
                "original_sha256": digest(original),
                "patched_sha256": digest(replacement),
            })
        (BACKUP / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        os.sync()

    for relative, _, replacement in records:
        replace_file(CRATE / relative, replacement)
    os.sync()
    print(f"patched {sum(FILES.values())} derive declarations in {CRATE}")
    print(BACKUP / "manifest.json")


def restore() -> None:
    if not BACKUP.is_dir():
        raise RuntimeError(f"backup missing: {BACKUP}")
    records = saved_sources()
    for relative, original, _ in records:
        replace_file(CRATE / relative, original)
    os.sync()
    print(f"restored original crate source from {BACKUP}")


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in {"apply", "restore"}:
        raise SystemExit("usage: patch-secret-service-derive.py apply|restore")
    if sys.argv[1] == "apply":
        apply()
    else:
        restore()

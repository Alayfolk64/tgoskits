"""Exercise the actual cpio assembly and the root-driver preflight."""

import gzip
import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path

APP = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "linux_profile_initramfs", APP / "prepare-linux-profile-initramfs.py"
)
PREPARE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PREPARE)


class InitramfsPreparationTest(unittest.TestCase):
    def test_keeps_init_and_installs_matching_driver_and_dependency_index(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            staging = Path(temporary)
            original, modules = self.prepare_inputs(staging)
            PREPARE.build_initramfs(original, modules, staging)
            archive = gzip.decompress((staging / "initramfs.img").read_bytes())
            for name, expected in (
                ("init", b"upstream init fixture\n"),
                (f"lib/modules/{PREPARE.VERSION}/modules.dep", b"complete dependency fixture\n"),
                (f"lib/modules/{PREPARE.VERSION}/kernel/drivers/nvme/host/nvme.ko", b"nvme fixture"),
                (f"lib/modules/{PREPARE.VERSION}/kernel/fs/ext4/ext4.ko", b"ext4 fixture"),
            ):
                result = subprocess.run(
                    ["cpio", "-i", "--to-stdout", name], input=archive,
                    stdout=subprocess.PIPE, check=True,
                )
                self.assertEqual(result.stdout, expected)

    def test_missing_root_driver_fails_before_extracting(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            staging = Path(temporary)
            original, modules = self.prepare_inputs(staging)
            (modules / "kernel/fs/ext4/ext4.ko").unlink()
            with self.assertRaisesRegex(ValueError, "required root driver is missing"):
                PREPARE.build_initramfs(original, modules, staging)
            self.assertFalse((staging / "root").exists())

    @staticmethod
    def prepare_inputs(staging):
        source = staging / "source"
        source.mkdir()
        (source / "init").write_bytes(b"upstream init fixture\n")
        old_modules = source / "lib/modules" / PREPARE.VERSION
        old_modules.mkdir(parents=True)
        (old_modules / "modules.dep").write_bytes(b"incomplete original dependency fixture\n")
        names = ["init", *(str(path.relative_to(source)) for path in sorted((source / "lib").rglob("*")))]
        archive = subprocess.run(
            ["cpio", "-o", "-H", "newc"], cwd=source,
            input=("\n".join(names) + "\n").encode(), stdout=subprocess.PIPE, check=True,
        ).stdout
        original = staging / "original.img"
        original.write_bytes(gzip.compress(archive))
        modules = staging / "modules"
        modules.mkdir()
        (modules / "modules.dep").write_bytes(b"complete dependency fixture\n")
        for name, contents in (
            ("kernel/drivers/nvme/host/nvme.ko", b"nvme fixture"),
            ("kernel/fs/ext4/ext4.ko", b"ext4 fixture"),
        ):
            path = modules / name
            path.parent.mkdir(parents=True)
            path.write_bytes(contents)
        return original, modules


if __name__ == "__main__":
    unittest.main()

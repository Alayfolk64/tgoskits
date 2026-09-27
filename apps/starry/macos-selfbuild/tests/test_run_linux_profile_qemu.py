"""Linux uses the same virtual hardware without touching the Starry disk."""

import importlib.util
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

APP = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("linux_profile_qemu", APP / "run-linux-profile-qemu.py")
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class LinuxComparisonRunnerTest(unittest.TestCase):
    def test_linux_uses_matching_hardware_and_its_own_disk(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            with patch.object(RUNNER.sys, "argv", ["run-linux-profile-qemu.py", str(run)]):
                with patch.object(RUNNER.subprocess, "run", return_value=SimpleNamespace(returncode=7)) as launch:
                    self.assertEqual(RUNNER.main(), 7)
            command = launch.call_args.args[0]
            for option, value in (
                ("-accel", "tcg,thread=multi"),
                ("-machine", "virt,gic-version=3"),
                ("-cpu", "cortex-a53"),
                ("-smp", "8"),
                ("-m", "8192M"),
                ("-kernel", str(run / "vmlinuz")),
                ("-initrd", str(run / "initramfs.img")),
            ):
                self.assertEqual(command[command.index(option) + 1], value)
            self.assertEqual(command.count("-drive"), 1)
            self.assertEqual(command[command.index("-drive") + 1],
                             f"id=disk0,if=none,format=raw,file={run / 'rootfs.img'}")
            self.assertIn("nvme,drive=disk0,serial=tgoskits,max_ioqpairs=64,msix_qsize=65", command)
            self.assertNotIn("-gdb", command)
            self.assertNotIn("-S", command)
            self.assertEqual(launch.call_args.kwargs["cwd"], run)

    def test_previous_log_is_preserved_without_launching(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            (run / "qemu.log").write_text("previous evidence\n")
            self.assert_rejected(run, "refusing to overwrite")
            self.assertEqual((run / "qemu.log").read_text(), "previous evidence\n")

    def test_missing_initramfs_is_rejected_without_launching(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            (run / "initramfs.img").unlink()
            self.assert_rejected(run, "artifact is missing")

    def test_readonly_frozen_disk_is_rejected_without_launching(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            (run / "rootfs.img").chmod(0o444)
            self.assert_rejected(run, "not owner-writable")

    def test_hardlinked_disk_is_rejected_without_launching(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            (run / "shared.img").hardlink_to(run / "rootfs.img")
            self.assert_rejected(run, "independent copy")

    def assert_rejected(self, run, reason):
        with patch.object(RUNNER.sys, "argv", ["run-linux-profile-qemu.py", str(run)]):
            with patch.object(RUNNER.subprocess, "run") as launch:
                with self.assertRaisesRegex(SystemExit, reason):
                    RUNNER.main()
        launch.assert_not_called()

    @staticmethod
    def prepare_run(temporary):
        run = Path(temporary)
        for name in ("vmlinuz", "initramfs.img", "rootfs.img"):
            (run / name).write_bytes(b"prepared fixture")
        return run


if __name__ == "__main__":
    unittest.main()

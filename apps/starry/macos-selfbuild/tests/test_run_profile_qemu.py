"""Keep saved-kernel diagnosis faithful to the measured QEMU configuration."""

import importlib.util
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

APP = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("profile_qemu", APP / "run-profile-qemu.py")
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


class SavedKernelRunnerTest(unittest.TestCase):
    def test_debug_uses_a_local_socket_without_stopping_boot(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            with patch.dict("os.environ", {"STARRY_PROFILE_DEBUG": "1"}):
                with patch.object(RUNNER.sys, "argv", ["run-profile-qemu.py", str(run)]):
                    with patch.object(RUNNER.subprocess, "run", return_value=SimpleNamespace(returncode=0)) as launch:
                        self.assertEqual(RUNNER.main(), 0)
            command = launch.call_args.args[0]
            self.assertIn("-gdb", command)
            self.assertIn("chardev:debug0", command)
            self.assertIn("socket,path=gdb.sock,server=on,wait=off,id=debug0", command)
            self.assertNotIn("-S", command)
            self.assertEqual(launch.call_args.kwargs["cwd"], run)
            self.assertEqual(command[command.index("-kernel") + 1], str(run / "starryos.bin"))

    def test_default_measurement_has_no_debug_server_and_preserves_failure(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            with patch.dict("os.environ", {"STARRY_PROFILE_DEBUG": "0"}):
                with patch.object(RUNNER.sys, "argv", ["run-profile-qemu.py", str(run)]):
                    with patch.object(RUNNER.subprocess, "run", return_value=SimpleNamespace(returncode=7)) as launch:
                        self.assertEqual(RUNNER.main(), 7)
            self.assertNotIn("-gdb", launch.call_args.args[0])

    def test_invalid_debug_setting_does_not_launch_qemu(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            with patch.dict("os.environ", {"STARRY_PROFILE_DEBUG": "invalid"}):
                with patch.object(RUNNER.sys, "argv", ["run-profile-qemu.py", str(run)]):
                    with patch.object(RUNNER.subprocess, "run") as launch:
                        with self.assertRaisesRegex(SystemExit, "STARRY_PROFILE_DEBUG"):
                            RUNNER.main()
            launch.assert_not_called()

    def test_existing_serial_evidence_is_not_overwritten(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            run = self.prepare_run(temporary)
            (run / "qemu.log").write_text("previous experiment\n")
            with patch.object(RUNNER.sys, "argv", ["run-profile-qemu.py", str(run)]):
                with patch.object(RUNNER.subprocess, "run") as launch:
                    with self.assertRaisesRegex(SystemExit, "refusing to overwrite"):
                        RUNNER.main()
            launch.assert_not_called()

    @staticmethod
    def prepare_run(temporary):
        run = Path(temporary)
        (run / "starryos.elf").write_bytes(b"saved ELF")
        (run / "starryos.bin").write_bytes(b"saved kernel")
        return run


if __name__ == "__main__":
    unittest.main()

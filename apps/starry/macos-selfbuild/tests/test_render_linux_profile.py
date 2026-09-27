"""Validate Linux trace accounting independently of successful compilation."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

APP = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("linux_profile", APP / "render-linux-profile.py")
PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROFILE)


class LinuxProfileTest(unittest.TestCase):
    def test_stack_order_idle_and_unknown_are_preserved(self):
        cpu, active, wait = PROFILE.folded_stacks({
            "@cpu": {"\nleaf+12\nroot+40\n,rustc": 7,
                     "\ndefault_idle_call+4\n,swapper/0": 13, ",rustc": 5},
            "@offcpu": {"\nschedule+4\nfutex_wait+16\n,rustc": "123456789"},
        })
        self.assertEqual(sum(cpu.values()), 25)
        self.assertEqual(sum(active.values()), 12)
        self.assertEqual(active["task:rustc;root;leaf"], 7)
        self.assertEqual(active["task:rustc;[no-kernel-stack]"], 5)
        self.assertEqual(wait["task:rustc;futex_wait;schedule"], 123456789)

    def test_proc_stat_does_not_double_count_guest_or_call_iowait_busy(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            before, after = Path(temporary) / "before", Path(temporary) / "after"
            before.write_text("cpu 10 0 5 100 10 0 0 0 5 0\n")
            after.write_text("cpu 30 0 15 150 30 0 0 0 15 0\n")
            result = PROFILE.stat_delta(before, after)
            self.assertEqual(result["total_ticks"], 100)
            self.assertEqual(result["busy_percent"], 30)

    def test_cpu_idle_flag_keeps_per_cpu_counts(self):
        result = PROFILE.cpu_states({"0,0": 7, "0,1": 13, "1,true": 20})
        self.assertEqual(result["0"], {"total": 20, "active": 7, "idle": 13})
        self.assertEqual(result["1"]["idle"], 20)

    def test_loss_and_missing_end_are_rejected(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            trace = Path(temporary) / "trace"
            trace.write_text(json.dumps({"type": "lost_events", "count": 3}) + "\n")
            with self.assertRaisesRegex(ValueError, "lost_events"):
                PROFILE.read_trace(trace)
            trace.write_text(json.dumps({"type": "map", "data": {"@cpu": {}}}) + "\n")
            with self.assertRaisesRegex(ValueError, "missing.*END"):
                PROFILE.read_trace(trace)

    def test_duplicate_maps_are_not_silently_double_counted(self):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            trace = Path(temporary) / "trace"
            record = json.dumps({"type": "map", "data": {"@cpu": {}}}) + "\n"
            trace.write_text(record * 2)
            with self.assertRaisesRegex(ValueError, "duplicate"):
                PROFILE.read_trace(trace)


if __name__ == "__main__":
    unittest.main()

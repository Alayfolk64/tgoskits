"""Check the unit contract passed to the external flamegraph renderer."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

APP = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("kernel_profile", APP / "render-kernel-profile.py")
PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROFILE)


class FlamegraphUnitsTest(unittest.TestCase):
    def test_wait_graph_reports_nanoseconds_instead_of_cpu_samples(self):
        self.check_unit("mutex-wait.folded", "nanoseconds")

    def test_cpu_graph_reports_samples(self):
        self.check_unit("cpu.folded", "samples")

    def test_active_cpu_graph_reports_samples(self):
        self.check_unit("cpu-active.folded", "samples")

    def check_unit(self, filename, expected):
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            directory = Path(temporary)
            folded = directory / filename
            folded.write_text("root;work 100\n")
            with patch.object(PROFILE.subprocess, "run") as render:
                PROFILE.render_flamegraph(
                    Path("flamegraph.pl"), folded, directory / "graph.svg", "test"
                )
            command = render.call_args.args[0]
            self.assertIn("--countname", command)
            self.assertEqual(command[command.index("--countname") + 1], expected)


class ActiveCpuSummaryTest(unittest.TestCase):
    def test_active_graph_excludes_idle_but_summary_preserves_its_cost(self):
        rows = [(0, "idle", 70), (0, "rustc", 10), (1, "idle-worker", 20)]
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            directory = Path(temporary)
            raw = directory / "kernel-profile.raw"
            records = ["STARRY_PROFILE_V1 sample_hz=10"]
            records.extend(
                f"STARRY_CPU phase=1 cpu={cpu} samples={samples} "
                f"task={task.encode().hex()} stack=user"
                for cpu, task, samples in rows
            )
            raw.write_text("\n".join(records) + "\n")
            args = SimpleNamespace(
                raw=raw, elf=directory / "unused.elf", out=directory / "rendered",
                addr2line="addr2line", flamegraph=None,
            )
            with patch.object(PROFILE, "parse_args", return_value=args):
                PROFILE.main()
            active = (args.out / "cpu-active.folded").read_text()
            self.assertNotIn(";task:idle;", active)
            self.assertIn(";task:idle-worker;", active)
            self.assertEqual(sum(int(line.rsplit(" ", 1)[1]) for line in active.splitlines()), 30)
            self.assertIn(";task:idle;", (args.out / "cpu.folded").read_text())
            cpu = json.loads((args.out / "summary.json").read_text())["cpu"]
            self.assertEqual(cpu["total_samples"], 100)
            self.assertEqual(cpu["active_samples"], 30)
            self.assertEqual(cpu["idle_samples"], 70)
            self.assertEqual(cpu["active_percent"], 30.0)
            self.assertEqual(cpu["active_leaf_top"], [
                {"stack": "[user-space]", "value": 30, "percent": 100.0},
            ])


class OwnerLatencySummaryTest(unittest.TestCase):
    def test_owner_totals_merge_callers_and_keep_fault_overlap_explicit(self):
        symbols = {
            "a": "Ext4Filesystem::lock",
            "b": "Ext4Filesystem::read_inode",
            "c": "starry_kernel::task::user::handle_user_page_fault",
            "d": "Ext4Filesystem::sync_to_disk",
        }
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            directory = Path(temporary)
            raw = directory / "kernel-profile.raw"
            raw.write_text(
                "STARRY_PROFILE_V1 sample_hz=10\n"
                "STARRY_WAIT phase=1 cpu=0 event=8 count=1 total_ns=30 "
                "max_ns=30 task=7275737463 stack=a;b;c\n"
                "STARRY_WAIT phase=1 cpu=1 event=8 count=1 total_ns=20 "
                "max_ns=20 task=6c64 stack=a;b\n"
                "STARRY_WAIT phase=1 cpu=1 event=8 count=1 total_ns=50 "
                "max_ns=50 task=6c64 stack=a;d\n"
            )
            args = SimpleNamespace(
                raw=raw, elf=directory / "unused.elf", out=directory / "rendered",
                addr2line="addr2line", flamegraph=None,
            )
            with patch.object(PROFILE, "parse_args", return_value=args):
                with patch.object(PROFILE, "symbolize", return_value=symbols):
                    PROFILE.main()
            event = json.loads((args.out / "summary.json").read_text())["wait"]["ext4-lock-hold"]
            self.assertEqual(event["leaf_top"], [
                {"stack": symbols["a"], "value": 100, "percent": 100.0},
            ])
            callers = {row["stack"]: row["value"] for row in event["caller_top"]}
            self.assertEqual(callers, {symbols["b"]: 50, symbols["d"]: 50})
            # These 30 ns are contained in read_inode's 50 ns, not additional time.
            self.assertEqual(event["page_fault_total_ns"], 30)
            self.assertEqual(event["total_ns"], 100)

    def test_new_owner_and_flush_events_keep_separate_nanosecond_graphs(self):
        events = {
            7: "ext4-lock-wait", 8: "ext4-lock-hold", 9: "block-flush-sync",
            10: "block-dispatch", 11: "block-completion", 12: "block-resume",
            13: "block-write-dispatch", 14: "block-write-completion",
            15: "block-write-resume", 16: "block-flush-dispatch",
            17: "block-flush-completion", 18: "block-flush-resume",
        }
        with tempfile.TemporaryDirectory(dir=APP.parents[2] / "tmp") as temporary:
            directory = Path(temporary)
            raw = directory / "kernel-profile.raw"
            records = ["STARRY_PROFILE_V1 sample_hz=10"]
            records.extend(
                f"STARRY_WAIT phase=1 cpu=3 event={event} count=2 "
                f"total_ns={event * 100} max_ns={event * 60} task=6c64 stack=user"
                for event in events
            )
            raw.write_text("\n".join(records) + "\n")
            args = SimpleNamespace(
                raw=raw, elf=directory / "unused.elf", out=directory / "rendered",
                addr2line="addr2line", flamegraph=Path("flamegraph.pl"),
            )
            with patch.object(PROFILE, "parse_args", return_value=args):
                with patch.object(PROFILE.subprocess, "run") as render:
                    PROFILE.main()
            summary = json.loads((args.out / "summary.json").read_text())["wait"]
            for event, name in events.items():
                self.assertIn(name, summary)
                self.assertEqual(summary[name]["sampled_count"], 2)
                self.assertEqual(summary[name]["total_ns"], event * 100)
                self.assertEqual(summary[name]["max_ns"], event * 60)
                self.assertEqual(summary[name]["by_cpu_total_ns"], {"3": event * 100})
                folded = (args.out / f"{name}.folded").read_text()
                self.assertEqual(folded, f"starry-wait:{name};task:ld;[user-space] {event * 100}\n")
            self.assertEqual(render.call_count, len(events))
            for call in render.call_args_list:
                command = call.args[0]
                self.assertEqual(command[command.index("--countname") + 1], "nanoseconds")


if __name__ == "__main__":
    unittest.main()

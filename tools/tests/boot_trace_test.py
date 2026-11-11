import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("boot_trace", ROOT / "tools/boot-trace.py")
boot_trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boot_trace)

# init starts a service whose property ends init's wait; an app host thread
# waits for a stage another thread runs, then sets sys.boot_completed.
TRACE = "\n".join("\t".join(map(str, line)) for line in [
    (0, 1000, "init", "[early-init] start a", ""),
    (500, 500, "svc/a", "start pid 7", "lane:init"),
    (1000, 9000, "init", "wait", "property:a.ready=true"),
    (8000, 8000, "svc/a", "set a.ready=true", ""),
    (9000, 9800, "init", "[late-init] setprop b.ready 1", ""),
    (9600, 9600, "pack", "started", "lane:init"),
    (9700, 20000, "pack", "wait", "property:b.ready=1"),
    (20000, 40000, "pack", "package construction", ""),
    (12000, 12000, "boot", "agent attached", ""),
    (12000, 40500, "boot", "wait", "stage:package construction"),
    (40500, 50000, "boot", "finish booting", ""),
    (45000, 45000, "boot", "set sys.boot_completed=1", ""),
]) + "\n"


class CriticalPath(unittest.TestCase):
    def test_the_path_follows_property_lane_and_stage_edges(self):
        trace = boot_trace.Trace(boot_trace.parse(TRACE))
        target = trace.latest_set("sys.boot_completed=1", float("inf"))
        steps = trace.critical_path(target)
        self.assertEqual([(lane, label) for _, _, lane, label in steps], [
            ("init", "[early-init] start a"),
            ("svc/a", "start pid 7 on lane:init"),
            ("svc/a", "(running)"),
            ("svc/a", "set a.ready=true"),
            ("init", "wait on property:a.ready=true"),
            ("init", "[late-init] setprop b.ready 1"),
            ("pack", "wait on property:b.ready=1"),
            ("pack", "package construction"),
            ("boot", "wait on stage:package construction"),
            ("boot", "finish booting"),
            ("boot", "set sys.boot_completed=1"),
        ])

    def test_a_wait_without_a_recorded_set_goes_on_along_its_lane(self):
        trace = boot_trace.Trace(boot_trace.parse(
            "0\t10\tx\tfirst\t\n10\t90\tx\twait\tproperty:never=1\n90\t90\tx\tset done=1\t\n"))
        steps = trace.critical_path(trace.latest_set("done=1", float("inf")))
        self.assertEqual([label for *_, label in steps],
                         ["first", "wait on property:never=1", "set done=1"])


if __name__ == "__main__":
    unittest.main()

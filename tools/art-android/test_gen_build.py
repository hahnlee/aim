#!/usr/bin/env python3
"""Host checks of gen_build.py's Android.bp evaluation; no NDK or image."""

import os
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bp_query  # noqa: E402
import gen_build  # noqa: E402

BP = """
filegroup {
    name: "messages",
    srcs: ["message/*.proto"],
}
enums = ["enums/app/**/*.proto"]
cc_library_host_shared {
    name: "protos",
    srcs: ["atoms.proto", ":messages", ":generated"] + enums,
}
genrule {
    name: "writer.h",
    tools: ["stats-log-api-gen"],
    cmd: "$(location stats-log-api-gen) --header $(genDir)/writer.h --module art",
    out: ["writer.h"],
}
"""


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        for path in ("stats/atoms.proto", "stats/message/a.proto",
                     "stats/enums/app/b.proto", "stats/enums/app/job/c.proto"):
            os.makedirs(os.path.dirname(os.path.join(self.root, path)), exist_ok=True)
            open(os.path.join(self.root, path), "w").close()
        with open(os.path.join(self.root, "stats/Android.bp"), "w") as handle:
            handle.write(BP)
        self.build = gen_build.Build.__new__(gen_build.Build)
        self.build.src_root = self.root
        self.build.gen = os.path.join(self.root, "gen")
        self.build.lines = []
        self.build.modules = bp_query.load(self.root, ["stats/Android.bp"])

    def tearDown(self):
        self.tmp.cleanup()

    def test_srcs_expand_filegroups_and_recursive_globs(self):
        # Soong expands a filegroup reference and `**` crosses directories;
        # references to other module kinds (genrule outputs) stay out.
        got = [os.path.relpath(p, self.root) for p in self.build.srcs("protos")]
        self.assertEqual(got, ["stats/atoms.proto", "stats/message/a.proto",
                               "stats/enums/app/b.proto", "stats/enums/app/job/c.proto"])

    def test_genrule_runs_its_own_command_in_the_sandbox(self):
        outputs = self.build.genrule("writer.h", ("/t/gen", "/t/sandbox", ["/t/sandbox/data"]))
        gen_dir = os.path.join(self.root, "gen/genrule/writer.h")
        self.assertEqual(outputs, [os.path.join(gen_dir, "writer.h")])
        self.assertEqual(self.build.lines, [
            f"build {gen_dir}/writer.h: genrule | /t/gen /t/sandbox/data",
            f"  cmd = cd /t/sandbox && /t/gen --header {gen_dir}/writer.h --module art",
        ])


if __name__ == "__main__":
    unittest.main()

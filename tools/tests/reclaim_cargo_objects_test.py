import importlib.util
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("reclaim", ROOT / "tools/reclaim-cargo-objects.py")
reclaim = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reclaim)


class Reclaim(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(dir=ROOT / "target")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.project = self.root / "project"
        self.project.mkdir()
        (self.project / "Cargo.toml").write_text(
            '[workspace]\n[package]\nname="reclaim-probe"\nversion="0.0.0"\nedition="2024"\n')
        (self.project / "src").mkdir()
        (self.project / "src/main.rs").write_text('fn main() { println!("retained"); }\n')
        self.target = self.root / "target"
        self.profile = self.target / "debug"
        self.command = ["cargo", "build", "--offline", "--manifest-path",
                        str(self.project / "Cargo.toml"), "--target-dir", str(self.target)]

    def build(self):
        subprocess.run(self.command, check=True, capture_output=True, timeout=60)

    def object(self):
        obj = self.profile / "deps/stale-generation.rcgu.o"
        obj.write_bytes(b"leftover link object")
        return obj

    def test_completed_cargo_profile_preserves_all_other_outputs(self):
        self.build()
        obj = self.object()
        retained = [self.profile / "reclaim-probe", self.profile / "deps/retained.rlib",
                    self.profile / "incremental/retained", self.target / "evidence/receipt",
                    self.profile / "deps/retained.o"]
        for path in retained[1:]:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(b"retained")
        evidence_link = self.target / "evidence/object-copy"
        evidence_link.hardlink_to(obj)
        retained.append(evidence_link)
        before = {path: path.read_bytes() for path in retained}
        objects = list((self.profile / "deps").glob("*.rcgu.o"))
        size = sum(path.stat().st_size for path in objects)
        self.assertEqual(reclaim.reclaim(self.profile, root=self.root)["objects"], len(objects))
        self.assertTrue(obj.exists())
        result = reclaim.reclaim(self.profile, apply=True, root=self.root)
        self.assertEqual(result["objects"], len(objects))
        self.assertEqual(result["logical_bytes"], size)
        self.assertFalse(obj.exists())
        self.assertEqual(before, {path: path.read_bytes() for path in retained})
        with (self.project / "src/main.rs").open("a") as source:
            source.write("// force an incremental rebuild after reclamation\n")
        self.build()
        output = subprocess.check_output([self.profile / "reclaim-probe"], timeout=10)
        self.assertEqual(output, b"retained\n")

    def test_actual_busy_cargo_profile_refuses_nonblocking(self):
        self.build()
        obj = self.object()
        (self.project / "build.rs").write_text('''
use std::{path::Path, thread, time::{Duration, Instant}};
fn main() {
    std::fs::write("entered", "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !Path::new("release").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
}
''')
        cargo = subprocess.Popen(self.command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 20
            while not (self.project / "entered").exists():
                self.assertIsNone(cargo.poll(), "Cargo exited before build-script barrier")
                self.assertLess(time.monotonic(), deadline)
                time.sleep(0.01)
            for apply in (False, True):
                with self.assertRaises(BlockingIOError):
                    reclaim.reclaim(self.profile, apply=apply, root=self.root)
            self.assertTrue(obj.exists())
        finally:
            (self.project / "release").touch()
            try:
                stdout, stderr = cargo.communicate(timeout=40)
            except subprocess.TimeoutExpired:
                cargo.kill()
                cargo.communicate()
                raise
        self.assertEqual(cargo.returncode, 0, (stdout, stderr))
        self.assertGreaterEqual(reclaim.reclaim(self.profile, apply=True, root=self.root)["objects"], 1)

    def test_no_symlink_or_unrelated_directory_traversal(self):
        self.build()
        obj = self.object()
        outside = self.root / "outside"
        outside.mkdir()
        victim = outside / "victim.rcgu.o"
        victim.write_bytes(b"preserve")
        (self.profile / "deps/link.rcgu.o").symlink_to(victim)
        (self.profile / "deps/directory.rcgu.o").mkdir()
        (self.target / "linked").symlink_to(self.profile, target_is_directory=True)
        unrelated = self.target / "evidence"
        (unrelated / "deps").mkdir(parents=True)
        (unrelated / "deps/evidence.rcgu.o").write_bytes(b"preserve")
        for path in (outside, self.target, self.target / "../outside", self.target / "linked", unrelated):
            with self.assertRaises((OSError, ValueError)):
                reclaim.reclaim(path, apply=True, root=self.root)
        for name in ("deps", ".cargo-lock"):
            path = self.profile / name
            held = self.profile / (name + ".held")
            path.rename(held)
            path.symlink_to(held, target_is_directory=name == "deps")
            try:
                with self.assertRaises(OSError):
                    reclaim.reclaim(self.profile, apply=True, root=self.root)
            finally:
                path.unlink()
                held.rename(path)
        self.assertTrue(obj.exists())
        self.assertFalse((unrelated / ".cargo-lock").exists())
        self.assertEqual((unrelated / "deps/evidence.rcgu.o").read_bytes(), b"preserve")
        reclaim.reclaim(self.profile, apply=True, root=self.root)
        self.assertEqual(victim.read_bytes(), b"preserve")
        self.assertTrue((self.profile / "deps/link.rcgu.o").is_symlink())
        self.assertTrue((self.profile / "deps/directory.rcgu.o").is_dir())


if __name__ == "__main__":
    unittest.main()

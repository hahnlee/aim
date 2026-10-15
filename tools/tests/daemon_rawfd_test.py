"""Pinned SDK backend regression for distinct raw FD and explicit PFD methods."""
import argparse
import importlib.util
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("daemon_aidl", ROOT / "tools/lib/daemon_aidl.py")
daemon = importlib.util.module_from_spec(spec)
spec.loader.exec_module(daemon)
AIDL = None


class RawFdGeneration(unittest.TestCase):
    def test_mixed_method_preserves_original_cpp_wire_and_distinct_rust_owners(self):
        original = """package fd;
interface IWire {
    void mixed(FileDescriptor raw, in ParcelFileDescriptor boxed, int following);
    FileDescriptor result();
}
"""
        staged, mapped = daemon.stage_raw_fds(original)
        self.assertTrue(mapped)
        with tempfile.TemporaryDirectory(prefix="aim-raw-fd-aidl-") as temporary:
            root = pathlib.Path(temporary)
            (root / "original/fd").mkdir(parents=True)
            (root / "staged/fd").mkdir(parents=True)
            declaration = root / "staged/dev/aim/binder/AimRawFileDescriptor.aidl"
            declaration.parent.mkdir(parents=True)
            declaration.write_text(daemon.RAW_FD_DECLARATION)
            (root / "original/fd/IWire.aidl").write_text(original)
            (root / "staged/fd/IWire.aidl").write_text(staged)
            subprocess.run([AIDL, "--lang=cpp", "-I", "original", "-o", "cpp", "-h", "headers",
                            "original/fd/IWire.aidl"], cwd=root, check=True, capture_output=True)
            subprocess.run([AIDL, "--lang=rust", "-I", "staged", "-o", "rust",
                            "staged/fd/IWire.aidl"], cwd=root, check=True, capture_output=True)
            cpp = (root / "cpp/fd/IWire.cpp").read_text()
            rust = (root / "rust/fd/IWire.rs").read_text()
            self.assertIn("writeUniqueFileDescriptor(raw)", cpp)
            self.assertIn("writeParcelable(boxed)", cpp)
            self.assertIn("_arg_raw: &crate::RawFileDescriptor", rust)
            self.assertIn("_arg_boxed: &binder::ParcelFileDescriptor", rust)
            self.assertIn("binder::Result<crate::RawFileDescriptor>", rust)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--aidl", required=True, help="Pinned SDK aidl executable")
    args = parser.parse_args()
    AIDL = str(pathlib.Path(args.aidl).resolve())
    unittest.main(argv=[__file__])

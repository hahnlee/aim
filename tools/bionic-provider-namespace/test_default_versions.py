import unittest
import pathlib
from default_versions import load_defaults
from extract_default_versions import defaults


class DefaultVersions(unittest.TestCase):
    def test_pinned_liblog_versions_are_symbol_specific(self):
        versions = load_defaults(pathlib.Path(__file__).parent)["liblog.so"]
        self.assertEqual(versions["__android_log_assert"], "LIBLOG")
        self.assertEqual(versions["__android_log_set_logger"], "LIBLOG_R")
        self.assertEqual(versions["__android_log_is_loggable"], "LIBLOG_M")
        self.assertEqual(versions["__android_log_is_loggable_len"], "LIBLOG_O")

    def test_default_not_import_hidden_or_compatibility_version(self):
        parsed = defaults("""
1: 0000 1 FUNC GLOBAL DEFAULT UND imported@LIBC
2: 0001 1 FUNC GLOBAL DEFAULT 15 strlen@@LIBC
3: 0002 1 FUNC GLOBAL DEFAULT 15 strlen@OLD
4: 0003 1 FUNC GLOBAL HIDDEN 15 private@@LIBC
5: 0004 1 OBJECT WEAK DEFAULT 15 plain
""")
        self.assertEqual(parsed, {"strlen": "LIBC", "plain": ""})

    def test_conflicting_defaults_fail(self):
        with self.assertRaises(ValueError):
            defaults("1: 0 1 FUNC GLOBAL DEFAULT 15 x@@V1\n2: 1 1 FUNC GLOBAL DEFAULT 15 x@@V2")

    def test_unrecognized_output_fails(self):
        with self.assertRaises(ValueError):
            defaults("not a dynamic symbol table")


if __name__ == "__main__":
    unittest.main()

import unittest
from metadata import boot_jars, fields


def string(key, value):
    encoded = value.encode()
    return bytes([key * 8 + 2, len(encoded)]) + encoded


def entry(path, kind=1, minimum="", maximum=""):
    body = string(1, path) + bytes([16, kind])
    if minimum:
        body += string(3, minimum)
    if maximum:
        body += string(4, maximum)
    return bytes([10, len(body)]) + body


class MetadataTests(unittest.TestCase):
    def test_filters_kind_and_sdk_preserving_order(self):
        data = entry("/system/a.jar") + entry("/system/b.jar", 3)
        data += entry("/system/c.jar", minimum="37")
        data += entry("/system/d.jar", maximum="35")
        data += entry("/system/e.jar", minimum="36")
        self.assertEqual(boot_jars(data), ["/system/a.jar", "/system/e.jar"])

    def test_rejects_bad_input(self):
        for data in (b"\x0a\xff", b"\x00", b"\x0a\x10abc"):
            with self.assertRaises(ValueError):
                list(fields(data))
        for path in ("../evil.jar", "/system/../evil.jar", "/system//a.jar"):
            with self.assertRaises(ValueError):
                boot_jars(entry(path))
        with self.assertRaises(ValueError):
            boot_jars(entry("/system/a.jar") * 2)


if __name__ == "__main__":
    unittest.main()

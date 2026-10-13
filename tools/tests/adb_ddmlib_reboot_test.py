import importlib.util
import socket
import threading
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "adb_ddmlib_reboot", ROOT / "tools" / "adb-ddmlib-reboot.py"
)
PROBE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROBE)


def frame(connection):
    length = int(PROBE.read_exact(connection, 4), 16)
    return PROBE.read_exact(connection, length)


class DdmlibRebootProtocolTest(unittest.TestCase):
    def test_selects_writes_reboot_and_closes_without_reading_reply(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        received = []
        errors = []

        def server():
            try:
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    connection.sendall(b"OKAY")
                    received.append(frame(connection))
                    connection.settimeout(2)
                    received.append(connection.recv(1))
            except Exception as error:
                errors.append(error)

        worker = threading.Thread(target=server)
        worker.start()
        PROBE.reboot_like_ddmlib(listener.getsockname(), "127.0.0.1:5637", 2)
        worker.join(3)
        listener.close()

        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])
        self.assertEqual(received, [b"host:transport:127.0.0.1:5637", b"reboot:", b""])

    def test_rejects_device_selection_before_writing_reboot(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        received = []

        def server():
            connection, _ = listener.accept()
            with connection:
                received.append(frame(connection))
                reason = b"device offline"
                connection.sendall(b"FAIL" + f"{len(reason):04X}".encode() + reason)

        worker = threading.Thread(target=server)
        worker.start()
        with self.assertRaisesRegex(RuntimeError, "device offline"):
            PROBE.reboot_like_ddmlib(listener.getsockname(), "serial", 2)
        worker.join(3)
        listener.close()
        self.assertEqual(received, [b"host:transport:serial"])

    def test_gate_requires_offline_reconnect_and_changed_boot_id(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(16)
        received = []
        errors = []
        old = b"11111111-1111-4111-8111-111111111111\n"
        new = b"22222222-2222-4222-8222-222222222222\n"

        def selected_service(service, output):
            connection, _ = listener.accept()
            with connection:
                received.append(frame(connection))
                connection.sendall(b"OKAY")
                received.append(frame(connection))
                self.assertEqual(received[-1], service)
                connection.sendall(b"OKAY" + output)

        def server():
            try:
                selected_service(
                    b"exec:/system/bin/cat /proc/sys/kernel/random/boot_id", old
                )
                # Exact ddmlib reboot connection.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    connection.sendall(b"OKAY")
                    received.append(frame(connection))
                    self.assertEqual(connection.recv(1), b"")
                # Offline is a qualified server response, not a dead server.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    reason = b"device offline"
                    connection.sendall(b"FAIL" + f"{len(reason):04X}".encode() + reason)
                # A normal host reconnect preparation rejection is retried.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    reason = b"service not ready"
                    connection.sendall(b"FAIL" + f"{len(reason):04X}".encode() + reason)
                # Explicit TCP-device reconnect.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    message = b"connected to serial"
                    connection.sendall(b"OKAY" + f"{len(message):04X}".encode() + message)
                # Availability after reconnect, followed by the new boot ID.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    connection.sendall(b"OKAY")
                # A selected transport can precede service readiness.  The
                # next full reconnect iteration must retry this rejection.
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    connection.sendall(b"OKAY")
                    received.append(frame(connection))
                    reason = b"service not ready"
                    connection.sendall(b"FAIL" + f"{len(reason):04X}".encode() + reason)
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    message = b"already connected to serial"
                    connection.sendall(b"OKAY" + f"{len(message):04X}".encode() + message)
                connection, _ = listener.accept()
                with connection:
                    received.append(frame(connection))
                    connection.sendall(b"OKAY")
                selected_service(
                    b"exec:/system/bin/cat /proc/sys/kernel/random/boot_id", new
                )
            except Exception as error:
                errors.append(error)

        worker = threading.Thread(target=server)
        worker.start()
        self.assertEqual(
            PROBE.run(listener.getsockname(), "serial", 2),
            (old.decode().strip(), new.decode().strip()),
        )
        worker.join(3)
        listener.close()

        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])
        self.assertIn(b"reboot:", received)
        self.assertIn(b"host:connect:serial", received)

    def test_arbitrary_selector_failure_is_not_offline(self):
        self.assertTrue(PROBE.unavailable_reason("serial", "device offline"))
        self.assertTrue(PROBE.unavailable_reason("serial", "device 'serial' not found"))
        for reason in ["unauthorized", "more than one device", "permission denied", ""]:
            self.assertFalse(PROBE.unavailable_reason("serial", reason))

    def test_only_lifecycle_preparation_rejections_are_retryable(self):
        for reason in [
            "device offline",
            "device 'serial' not found",
            "device not found",
            "no devices/emulators found",
            "device not ready",
            "service not ready",
        ]:
            self.assertTrue(PROBE.retryable_reason("serial", reason))
        for reason in ["unauthorized", "permission denied", "protocol fault", ""]:
            self.assertFalse(PROBE.retryable_reason("serial", reason))

    def test_gate_rejects_arbitrary_selector_failure(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(3)
        errors = []
        old = b"11111111-1111-4111-8111-111111111111\n"

        def server():
            try:
                connection, _ = listener.accept()
                with connection:
                    self.assertEqual(frame(connection), b"host:transport:serial")
                    connection.sendall(b"OKAY")
                    self.assertEqual(
                        frame(connection),
                        b"exec:/system/bin/cat /proc/sys/kernel/random/boot_id",
                    )
                    connection.sendall(b"OKAY" + old)
                connection, _ = listener.accept()
                with connection:
                    self.assertEqual(frame(connection), b"host:transport:serial")
                    connection.sendall(b"OKAY")
                    self.assertEqual(frame(connection), b"reboot:")
                    self.assertEqual(connection.recv(1), b"")
                connection, _ = listener.accept()
                with connection:
                    self.assertEqual(frame(connection), b"host:transport:serial")
                    reason = b"permission denied"
                    connection.sendall(b"FAIL" + f"{len(reason):04X}".encode() + reason)
            except Exception as error:
                errors.append(error)

        worker = threading.Thread(target=server)
        worker.start()
        with self.assertRaisesRegex(RuntimeError, "unexpected device selector failure"):
            PROBE.run(listener.getsockname(), "serial", 2)
        worker.join(3)
        listener.close()
        self.assertFalse(worker.is_alive())
        self.assertEqual(errors, [])


if __name__ == "__main__":
    unittest.main()

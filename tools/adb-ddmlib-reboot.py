#!/usr/bin/env python3
"""Exercise ddmlib's reboot smart-socket lifetime against an owned guest.

The caller owns the adb server and guest.  This probe selects the device,
writes ``reboot:``, and closes without reading a service response, matching
AdbHelper.reboot.  Success requires an unavailable transport transition,
reconnection, and a changed Linux boot ID.
"""

import argparse
import re
import socket
import time
import uuid


class AdbRejected(RuntimeError):
    def __init__(self, phase: str, reason: str):
        super().__init__(f"ADB {phase} rejected: {reason}")
        self.phase = phase
        self.reason = reason


class ProtocolError(RuntimeError):
    pass


def request(command: str) -> bytes:
    encoded = command.encode("utf-8")
    if len(encoded) > 0xFFFF:
        raise ValueError("ADB request exceeds the four-digit framing limit")
    return f"{len(encoded):04X}".encode("ascii") + encoded


def read_exact(connection: socket.socket, length: int) -> bytes:
    output = bytearray()
    while len(output) != length:
        chunk = connection.recv(length - len(output))
        if not chunk:
            raise EOFError("ADB server closed before its response was complete")
        output.extend(chunk)
    return bytes(output)


def read_status(connection: socket.socket) -> tuple[bool, str]:
    status = read_exact(connection, 4)
    if status == b"OKAY":
        return True, ""
    if status != b"FAIL":
        raise ProtocolError(f"invalid ADB server status {status!r}")
    raw_length = read_exact(connection, 4)
    try:
        length = int(raw_length, 16)
    except ValueError as error:
        raise ProtocolError(f"invalid ADB failure length {raw_length!r}") from error
    return False, read_exact(connection, length).decode("utf-8", "replace")


def select(connection: socket.socket, serial: str) -> tuple[bool, str]:
    connection.sendall(request(f"host:transport:{serial}"))
    return read_status(connection)


def available(address: tuple[str, int], serial: str, timeout: float) -> tuple[bool, str]:
    with socket.create_connection(address, timeout=timeout) as connection:
        connection.settimeout(timeout)
        return select(connection, serial)


def host_query(address: tuple[str, int], command: str, timeout: float) -> str:
    with socket.create_connection(address, timeout=timeout) as connection:
        connection.settimeout(timeout)
        connection.sendall(request(command))
        okay, reason = read_status(connection)
        if not okay:
            raise AdbRejected("host command", reason)
        raw_length = read_exact(connection, 4)
        try:
            length = int(raw_length, 16)
        except ValueError as error:
            raise ProtocolError(f"invalid ADB host response length {raw_length!r}") from error
        return read_exact(connection, length).decode("utf-8", "replace")


def service_output(
    address: tuple[str, int], serial: str, service: str, timeout: float
) -> bytes:
    with socket.create_connection(address, timeout=timeout) as connection:
        connection.settimeout(timeout)
        selected, reason = select(connection, serial)
        if not selected:
            raise AdbRejected("device selection", reason)
        connection.sendall(request(service))
        okay, reason = read_status(connection)
        if not okay:
            raise AdbRejected("service", reason)
        output = bytearray()
        while True:
            chunk = connection.recv(4096)
            if not chunk:
                return bytes(output)
            output.extend(chunk)
            if len(output) > 4096:
                raise ProtocolError("ADB boot ID response exceeds 4 KiB")


def boot_id(address: tuple[str, int], serial: str, timeout: float) -> str:
    text = service_output(
        address,
        serial,
        "exec:/system/bin/cat /proc/sys/kernel/random/boot_id",
        timeout,
    ).decode("ascii").strip()
    try:
        parsed = uuid.UUID(text)
    except ValueError as error:
        raise ProtocolError(f"invalid guest boot ID {text!r}") from error
    if str(parsed) != text.lower():
        raise ProtocolError(f"noncanonical guest boot ID {text!r}")
    return str(parsed)


def reboot_like_ddmlib(address: tuple[str, int], serial: str, timeout: float) -> None:
    connection = socket.create_connection(address, timeout=timeout)
    try:
        connection.settimeout(timeout)
        selected, reason = select(connection, serial)
        if not selected:
            raise RuntimeError(f"ADB device selection failed before reboot: {reason}")
        connection.sendall(request("reboot:"))
        # AdbHelper.reboot closes here.  It does not read the reboot service's
        # OKAY, payload, or EOF.
    finally:
        connection.close()


def unavailable_reason(serial: str, reason: str) -> bool:
    return reason == "device offline" or re.fullmatch(
        rf"device '{re.escape(serial)}' not found", reason
    ) is not None


def retryable_reason(serial: str, reason: str) -> bool:
    return unavailable_reason(serial, reason) or reason in {
        "device not found",
        "no devices/emulators found",
        "device not ready",
        "service not ready",
    }


def run(address: tuple[str, int], serial: str, timeout: float) -> tuple[str, str]:
    before = boot_id(address, serial, min(timeout, 5.0))
    reboot_like_ddmlib(address, serial, min(timeout, 5.0))

    deadline = time.monotonic() + timeout
    saw_offline = False
    last_reason = ""
    while time.monotonic() < deadline and not saw_offline:
        try:
            online, last_reason = available(address, serial, 1.0)
        except (ConnectionError, EOFError, OSError, TimeoutError) as error:
            raise RuntimeError(f"owned ADB server became unreadable: {error}") from error
        if not online:
            if not unavailable_reason(serial, last_reason):
                raise RuntimeError(f"unexpected device selector failure: {last_reason}")
            saw_offline = True
            break
        time.sleep(0.05)
    if not saw_offline:
        raise RuntimeError(f"device never became unavailable (last server response: {last_reason})")

    last_reconnect = ""
    while time.monotonic() < deadline:
        remaining = deadline - time.monotonic()
        try:
            last_reconnect = host_query(
                address, f"host:connect:{serial}", min(remaining, 2.0)
            )
        except AdbRejected as error:
            if not retryable_reason(serial, error.reason):
                raise
            last_reconnect = str(error)
            time.sleep(0.1)
            continue
        try:
            online, reason = available(address, serial, min(remaining, 2.0))
            if not online:
                if not retryable_reason(serial, reason):
                    raise RuntimeError(f"unexpected device selector failure: {reason}")
            else:
                try:
                    after = boot_id(address, serial, min(remaining, 5.0))
                except AdbRejected as error:
                    if not retryable_reason(serial, error.reason):
                        raise
                    last_reconnect = str(error)
                    time.sleep(0.1)
                    continue
                if after != before:
                    return before, after
        except (ConnectionError, EOFError, OSError, TimeoutError, UnicodeError):
            pass
        time.sleep(0.1)
    raise RuntimeError(
        f"device did not reconnect with a new boot ID; last connect response: {last_reconnect}"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--adb-server-port", type=int, required=True)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--timeout", type=float, default=300.0)
    args = parser.parse_args()
    if not 0 < args.adb_server_port <= 65535 or args.timeout <= 0:
        parser.error("port and timeout must be positive")
    before, after = run(("127.0.0.1", args.adb_server_port), args.serial, args.timeout)
    print(f"ddmlib reboot semantics: offline, reconnected, boot ID {before} -> {after}")


if __name__ == "__main__":
    main()

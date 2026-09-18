"""Host-only mocks for the read-only native sendmsg/recvmsg observer."""

import importlib.util
import pathlib
import unittest


_HELPER = pathlib.Path(__file__).with_name("lldb_socket_broker_readonly.py")
_SPEC = importlib.util.spec_from_file_location("socket_broker_helper", _HELPER)
SOCKET = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(SOCKET)


class _Error:
    def __init__(self):
        self.message = ""

    def Success(self):
        return not self.message

    def IsValid(self):
        return bool(self.message)

    def GetCString(self):
        return self.message


class _Lldb:
    SBError = _Error


class _Process:
    def __init__(self, memory, pid=321):
        self.memory = memory
        self.pid = pid

    def ReadMemory(self, address, size, error):
        data = self.memory.get(address)
        if data is None or len(data) != size:
            error.message = "missing %#x/%d" % (address, size)
            return b""
        return data

    def GetProcessID(self):
        return self.pid


class _Thread:
    def __init__(self, process):
        self.process = process

    def GetProcess(self):
        return self.process

    def GetThreadID(self):
        return 9

    def GetName(self):
        return "mock"


class _Reg:
    def __init__(self, value):
        self.value = value

    def IsValid(self):
        return True

    def GetValueAsUnsigned(self):
        return self.value


class _Frame:
    def __init__(self, process, registers):
        self.thread = _Thread(process)
        self.registers = registers

    def GetThread(self):
        return self.thread

    def FindRegister(self, name):
        return _Reg(self.registers.get(name, 0))

    def GetPC(self):
        return self.registers.get("pc", 0)


def _u(value, size):
    return value.to_bytes(size, "little")


def _message(control_address, control_length):
    header = bytearray(48)
    header[32:40] = _u(control_address, 8)
    header[40:44] = _u(control_length, 4)
    return bytes(header)


def _rights(fd):
    control = bytearray(16)
    control[0:4] = _u(16, 4)
    control[4:8] = _u(0xffff, 4)
    control[8:12] = _u(1, 4)
    control[12:16] = _u(fd & 0xffffffff, 4)
    return bytes(control)


class SocketObserverTests(unittest.TestCase):
    def setUp(self):
        SOCKET.lldb = _Lldb
        SOCKET.HOST_CALLS.clear()
        SOCKET.TRACE_EVENTS.clear()
        SOCKET.TRACE_EVENTS_DROPPED = 0
        self.old_cli = SOCKET.PEER_SNAPSHOT_CLI
        self.old_pids = SOCKET.PEER_SNAPSHOT_PIDS
        SOCKET.PEER_SNAPSHOT_CLI = ""
        SOCKET.PEER_SNAPSHOT_PIDS = ()

    def tearDown(self):
        SOCKET.PEER_SNAPSHOT_CLI = self.old_cli
        SOCKET.PEER_SNAPSHOT_PIDS = self.old_pids

    def test_rights_decode_preserves_fd_and_64bit_peer_identity(self):
        rights, records = SOCKET._decode_rights_control(_rights(0x7fffffff))
        self.assertEqual(rights, [0x7fffffff])
        self.assertEqual(records[0]["level"], 0xffff)

        class FakePopen:
            returncode = 0

            def __init__(self, command, **kwargs):
                self.command = command

            def communicate(self, timeout=None):
                header = "pid\tfd\tsoi_so\tsoi_pcb\tunsi_conn_so\tunsi_conn_pcb\ttype\tstate\n"
                local = "11\t5\t18446744073709551601\t18446744073709551602\t18446744073709551603\t18446744073709551604\t1\t290\n"
                peer = "22\t7\t18446744073709551603\t18446744073709551604\t18446744073709551601\t18446744073709551602\t1\t258\n"
                return (header + local + peer).encode(), b""

        old_popen = SOCKET.subprocess.Popen
        SOCKET.subprocess.Popen = FakePopen
        try:
            SOCKET.PEER_SNAPSHOT_CLI = "/reviewed/peers"
            SOCKET.PEER_SNAPSHOT_PIDS = (11, 22)
            rows, error = SOCKET._peer_snapshot_rows(11, 5)
        finally:
            SOCKET.subprocess.Popen = old_popen
        self.assertIsNone(error)
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["soi_so"], "18446744073709551601")
        self.assertEqual(rows[1]["unsi_conn_pcb"], "18446744073709551602")

    def test_malformed_and_truncated_control_fail_closed(self):
        with self.assertRaises(SOCKET.SnapshotError):
            SOCKET._decode_rights_control(b"\x08\x00\x00\x00" + b"\x00" * 8)
        with self.assertRaises(SOCKET.SnapshotError):
            SOCKET._decode_rights_control(b"\x00" * 13)

    def test_send_entry_records_carrier_and_rights(self):
        memory = {0x1000: _message(0x2000, 16),
                  0x2000: _rights(0x12345678)}
        process = _Process(memory)
        frame = _Frame(process, {"x0": 5, "x1": 0x1000, "x2": 0x80,
                                  "pc": 0x123456789abc})
        self.assertFalse(SOCKET.host_sendmsg_trace(frame, None, None))
        event = SOCKET.TRACE_EVENTS[-1]
        self.assertEqual(event["carrier_fd"], 5)
        self.assertEqual(event["rights"], [0x12345678])
        self.assertEqual(event["entry_pc"], 0x123456789abc & SOCKET.TRACE_ADDRESS_MASK)

    def test_recv_entry_ignores_prefilled_control_and_negative_return(self):
        memory = {0x1000: _message(0x2000, 16),
                  0x2000: _rights(0x87654321)}
        process = _Process(memory)
        entry = _Frame(process, {"x0": 5, "x1": 0x1000, "x2": 0,
                                  "pc": 0x100})
        self.assertFalse(SOCKET.host_recvmsg_trace(entry, None, None))
        self.assertEqual(SOCKET.TRACE_EVENTS[-1]["rights"], [])
        self.assertEqual(SOCKET.TRACE_EVENTS[-1]["control_capacity"], 16)

        failure = _Frame(process, {"x0": (1 << 64) - 1, "pc": 0x200})
        self.assertFalse(SOCKET.host_recvmsg_return_trace(failure, None, None))
        event = SOCKET.TRACE_EVENTS[-1]
        self.assertTrue(event["recv_failed"])
        self.assertEqual(event["result"], -1)
        self.assertEqual(event["rights"], [])

    def test_mismatched_return_does_not_pop_pending_call(self):
        memory = {0x1000: _message(0x2000, 0), 0x2000: b""}
        process = _Process(memory)
        entry = _Frame(process, {"x0": 5, "x1": 0x1000, "x2": 0})
        SOCKET.host_sendmsg_trace(entry, None, None)
        mismatch = _Frame(process, {"x0": 1})
        self.assertFalse(SOCKET.host_recvmsg_return_trace(mismatch, None, None))
        self.assertTrue(SOCKET.TRACE_EVENTS[-1]["mismatched_return"])
        self.assertIn(9, SOCKET.HOST_CALLS)
        success = _Frame(process, {"x0": 4})
        self.assertFalse(SOCKET.host_sendmsg_return_trace(success, None, None))
        self.assertNotIn(9, SOCKET.HOST_CALLS)


if __name__ == "__main__":
    unittest.main()

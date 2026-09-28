"""Host-only mocked tests for the bounded Root lifetime diagnostic."""

import importlib.util
import pathlib
import unittest


_HELPER = pathlib.Path(__file__).with_name("lldb_chrome_root_lifetime_readonly.py")
_SPEC = importlib.util.spec_from_file_location("root_lifetime_helper", _HELPER)
ROOT = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(ROOT)


class _Error:
    def __init__(self):
        self.message = ""

    def Success(self):
        return not self.message

    def GetCString(self):
        return self.message


class _Lldb:
    SBError = _Error


class _Process:
    def __init__(self, memory, pid=1234):
        self.memory = memory
        self.pid = pid

    def ReadMemory(self, address, size, error):
        data = self.memory.get(address)
        if data is None or len(data) != size:
            error.message = "mock missing %#x/%d" % (address, size)
            return b""
        return data

    def GetProcessID(self):
        return self.pid


class _Thread:
    def __init__(self, process, tid=77):
        self.process = process
        self.tid = tid

    def GetProcess(self):
        return self.process

    def GetThreadID(self):
        return self.tid

    def GetName(self):
        return "mock-thread"


class _Register:
    def __init__(self, value):
        self.value = value

    def GetValueAsUnsigned(self):
        return self.value


class _Frame:
    def __init__(self, process, registers=None, tid=77):
        self.thread = _Thread(process, tid)
        self.registers = registers or {}

    def GetThread(self):
        return self.thread

    def FindRegister(self, name):
        return _Register(self.registers.get(name, 0))


def _word(value, size=8):
    return value.to_bytes(size, "little")


class RootLifetimeTests(unittest.TestCase):
    def setUp(self):
        ROOT.lldb = _Lldb
        ROOT._reset_state()

    def test_nested_requests_are_lifo_and_correlated(self):
        memory = {
            0x1000: _word(0, 4), 0x1004: _word(1, 4),
            0x1050: _word(0xaaaa),
            0x2000: _word(0, 4), 0x2004: _word(1, 4),
            0x2050: _word(0xbbbb),
        }
        frame = _Frame(_Process(memory))
        outer = ROOT._request(frame, 0x1000, 0x10)
        inner = ROOT._request(frame, 0x2000, 0x20)
        self.assertEqual(ROOT.REQUESTS[77], [outer, inner])
        self.assertEqual((outer["capture_depth"], inner["capture_depth"]), (1, 2))
        self.assertIs(ROOT._pop_request(77), inner)
        self.assertIs(ROOT._pop_request(77), outer)
        self.assertNotIn(77, ROOT.REQUESTS)

    def test_output_result_recovers_exact_params_and_manager(self):
        memory = {
            0x2000: _word(0x3000),
            0x3000: _word(0, 4), 0x3004: _word(1, 4),
            0x3050: _word(0xcccc),
        }
        frame = _Frame(_Process(memory), {"x0": 0xfeed, "x21": 0x2000,
                                           "x28": 0x44})
        self.assertFalse(ROOT.output_result(frame, None, None))
        request = ROOT.REQUESTS[77][0]
        self.assertEqual(request["params"], 0x3000)
        self.assertEqual(request["manager"], 0x44)
        self.assertEqual(request["recovered_at"], "output_result")
        self.assertEqual(request["capture_epoch"], 1)
        self.assertEqual(ROOT.EVENTS[-1]["surface"], 0xfeed)

    def test_first_unrelated_close_does_not_consume_then_match_is_one_shot(self):
        memory = {
            0x4000: _word(0, 4), 0x4004: _word(1, 4),
            0x4050: _word(0xdddd),
        }
        frame = _Frame(_Process(memory), {"x0": 0, "x29": 0, "x30": 0x1234})
        request = ROOT._request(frame, 0x4000, 0x55)
        frame.registers["x0"] = 0xeeee
        self.assertFalse(ROOT.portal_close(frame, None, None))
        self.assertIn(0xdddd, ROOT.HANDLES)
        self.assertEqual(ROOT.EVENTS, [])

        frame.registers["x0"] = 0xdddd
        self.assertFalse(ROOT.portal_close(frame, None, None))
        self.assertNotIn(0xdddd, ROOT.HANDLES)
        self.assertEqual(ROOT.EVENTS[-1]["capture_epoch"],
                         request["capture_epoch"])
        event_count = len(ROOT.EVENTS)
        self.assertFalse(ROOT.portal_close(frame, None, None))
        self.assertEqual(len(ROOT.EVENTS), event_count)

    def test_untracked_nested_return_preserves_tracked_outer(self):
        memory = {
            0x1000: _word(0, 4), 0x1004: _word(1, 4),
            0x1050: _word(0xaaaa),
            0x2000: _word(2, 4), 0x2004: _word(3, 4),
            0x5008: _word(0x2000),
            0x6008: _word(0x1000),
        }
        process = _Process(memory)
        frame = _Frame(process, {"x0": 0x900, "x19": 0x20, "sp": 0x5000})
        outer = ROOT._request(frame, 0x1000, 0x20)
        self.assertIsNotNone(outer)

        # The nested params are not the tracked (0,1) Root request.
        self.assertIsNone(ROOT._request(frame, 0x2000, 0x30))
        self.assertFalse(ROOT.manager_result(frame, None, None))
        self.assertEqual(ROOT.REQUESTS[77], [outer])
        self.assertEqual([e for e in ROOT.EVENTS
                          if e["event"] == "root_manager_result"], [])

        frame.registers.update({"x19": 0x20, "sp": 0x6000})
        self.assertFalse(ROOT.manager_result(frame, None, None))
        self.assertNotIn(77, ROOT.REQUESTS)
        self.assertEqual(ROOT.EVENTS[-1]["params"], 0x1000)

    def test_output_same_manager_different_params_is_not_outer(self):
        memory = {
            0x1000: _word(0, 4), 0x1004: _word(1, 4),
            0x1050: _word(0xaaaa),
            0x2000: _word(0x3000),
            0x3000: _word(0, 4), 0x3004: _word(1, 4),
            0x3050: _word(0xbbbb),
        }
        process = _Process(memory)
        outer_frame = _Frame(process, {"x0": 0, "x1": 0, "x28": 0x44})
        outer = ROOT._request(outer_frame, 0x1000, 0x44)
        self.assertIsNotNone(outer)

        output = _Frame(process, {"x0": 0xfeed, "x21": 0x2000,
                                  "x28": 0x44})
        self.assertFalse(ROOT.output_result(output, None, None))
        self.assertEqual(ROOT.EVENTS[-1]["event"], "root_output_result")
        self.assertEqual(ROOT.EVENTS[-1]["params"], 0x3000)
        self.assertNotEqual(ROOT.EVENTS[-1]["params"], outer["params"])


if __name__ == "__main__":
    unittest.main()

"""Test-only Chromium 154 installed-ELF Root factory/portal lifetime tracing.

Set BIAS and install hardware breakpoints at the independently verified ELF
entries. Callbacks read registers/memory only and never invoke guest code.
"""
import json
import time
try:
    import lldb
except ImportError:  # Host-only mocked tests do not require LLDB.
    lldb = None

BIAS = 0
REQUESTS = {}
# REQUESTS is a per-thread LIFO stack.  A handle is owned by the newest
# capture only; this prevents a retired Router address from matching after
# allocator reuse.
HANDLES = set()
_HANDLE_OWNERS = {}
_CAPTURE_EPOCH = 0
EVENTS = []


def _reg(frame, name):
    return frame.FindRegister(name).GetValueAsUnsigned()


def _read(process, addr, size):
    err = lldb.SBError()
    data = process.ReadMemory(addr, size, err)
    if not err.Success() or len(data) != size:
        raise RuntimeError("read failed at %#x: %s" % (addr, err.GetCString()))
    return int.from_bytes(data, "little")


def _emit(frame, event, **values):
    process = frame.GetThread().GetProcess()
    item = dict(event=event, pid=process.GetProcessID(),
                tid=frame.GetThread().GetThreadID(),
                thread=frame.GetThread().GetName(),
                wall_ns=time.time_ns(), **values)
    EVENTS.append(item)
    del EVENTS[:-128]
    print(json.dumps(item, sort_keys=True))


def _reset_state():
    """Host-test helper; does not touch a target process."""
    global _CAPTURE_EPOCH
    REQUESTS.clear()
    HANDLES.clear()
    _HANDLE_OWNERS.clear()
    EVENTS.clear()
    _CAPTURE_EPOCH = 0


def _next_capture_epoch():
    global _CAPTURE_EPOCH
    _CAPTURE_EPOCH += 1
    return _CAPTURE_EPOCH


def _request(frame, params, manager, recovered_at=None):
    process = frame.GetThread().GetProcess()
    ident = (_read(process, params, 4), _read(process, params + 4, 4))
    if ident != (0, 1):
        return None
    handle = _read(process, params + 0x50, 8)
    tid = frame.GetThread().GetThreadID()
    stack = REQUESTS.setdefault(tid, [])
    request = dict(params=params, handle=handle, manager=manager,
                   recovered_at=recovered_at,
                   capture_epoch=_next_capture_epoch(),
                   capture_depth=len(stack) + 1)
    stack.append(request)
    if handle:
        _HANDLE_OWNERS[handle] = request
        HANDLES.add(handle)
    return request


def _current_request(tid, params, manager):
    for request in reversed(REQUESTS.get(tid, ())):
        if (request["params"] == params and
                request["manager"] == manager):
            return request
    return None


def _pop_request(tid):
    stack = REQUESTS.get(tid)
    if not stack:
        return None
    request = stack.pop()
    if not stack:
        del REQUESTS[tid]
    return request


def manager_entry(frame, loc, internal):
    try:
        process = frame.GetThread().GetProcess()
        params = _read(process, _reg(frame, "x1"), 8)
        request = _request(frame, params, _reg(frame, "x0"))
        if request is None:
            return False
        _emit(frame, "root_manager_entry", **request)
    except Exception as exc:
        _emit(frame, "root_trace_error", boundary="manager_entry", error=str(exc))
    return False


def output_result(frame, loc, internal):
    try:
        process = frame.GetThread().GetProcess()
        tid = frame.GetThread().GetThreadID()
        # x21 is the live &params at the output boundary. Always read it;
        # matching only M could incorrectly attribute an outer request.
        params = _read(process, _reg(frame, "x21"), 8)
        manager = _reg(frame, "x28")
        request = _current_request(tid, params, manager)
        if request is None:
            # Recovery remains exact: _request validates (0,1) and records
            # this P/M pair rather than borrowing an outer stack entry.
            request = _request(frame, params, manager, "output_result")
        if request:
            _emit(frame, "root_output_result", surface=_reg(frame, "x0"), **request)
    except Exception as exc:
        _emit(frame, "root_trace_error", boundary="output_result", error=str(exc))
    return False


def manager_result(frame, loc, internal):
    try:
        process = frame.GetThread().GetProcess()
        tid = frame.GetThread().GetThreadID()
        # Installed c5f5e10 keeps P at [sp+8] and M in x19 until return.
        params = _read(process, _reg(frame, "sp") + 8, 8)
        manager = _reg(frame, "x19")
        stack = REQUESTS.get(tid, ())
        request = stack[-1] if stack else None
        # Remove only an exact top match. An untracked nested manager must not
        # consume the tracked outer request.
        if (request is None or request["params"] != params or
                request["manager"] != manager):
            return False
        request = _pop_request(tid)
        _emit(frame, "root_manager_result", root=_reg(frame, "x0"), **request)
    except Exception as exc:
        _emit(frame, "root_trace_error", boundary="manager_result",
              error=str(exc))
    return False


def portal_close(frame, loc, internal):
    handle = _reg(frame, "x0")
    request = _HANDLE_OWNERS.pop(handle, None)
    if request is None:
        return False
    HANDLES.discard(handle)
    process = frame.GetThread().GetProcess()
    fp = _reg(frame, "x29")
    callers = [_reg(frame, "x30") & 0xffffffffffff]
    error = None
    for _ in range(16):
        if not fp:
            break
        try:
            previous = _read(process, fp, 8)
            callers.append(_read(process, fp + 8, 8) & 0xffffffffffff)
            if previous <= fp:
                break
            fp = previous
        except Exception as exc:
            error = str(exc)
            break
    _emit(frame, "root_portal_close", handle=handle, callers=callers,
          relative=[pc - BIAS for pc in callers], error=error,
          capture_epoch=request["capture_epoch"],
          capture_params=request["params"],
          capture_manager=request["manager"],
          capture_recovered_at=request["recovered_at"],
          capture_depth=request["capture_depth"])
    return False

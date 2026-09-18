"""Bounded, read-only LLDB snapshot of the native Darwin-art socket broker.

This is a diagnostic helper for the currently selected, already-stopped
process. The offsets below are a version-bound ABI snapshot, not a dynamic
ABI decoder; revalidate them whenever the broker source changes.
"""

import json
import os
import subprocess
import time

try:
    import lldb
except ImportError:  # Allows syntax-checking outside LLDB.
    lldb = None


MODULE_BASENAME = "libdarwin_art_runtime_graphics.dylib"
PROCESS_SYMBOL = "_ZN12_GLOBAL__N_19g_processE"
OWNER_CLOSE_SYMBOL = "_ZN12_GLOBAL__N_110OwnerCloseEPvyPi"

PROCESS_BROKER_OFFSET = 88
BROKER_SLOTS_OFFSET = 112
SLOT_STRIDE = 88
SLOT_COUNT = 1024
SLOT_GENERATION_OFFSET = 0
SLOT_RETIRED_OFFSET = 4
SLOT_LIVE_OFFSET = 5
SLOT_DESCRIPTION_OFFSET = 24
DESCRIPTION_KIND_OFFSET = 8
DESCRIPTION_CLOSE_OFFSET = 64
DESCRIPTION_OBJECT_OFFSET = 152
DESCRIPTION_REFS_OFFSET = 176
DESCRIPTION_ACTIVE_OFFSET = 184
HOST_FD_OFFSET = 0

TOKEN_MARKER = 0x40000000
SLOT_BITS = 10
GENERATION_MASK = (1 << 20) - 1
SOCKET_KIND = 4

# Host-only peer lookup is disabled unless the main debugger script supplies
# both an exact PID tuple and the already-reviewed CLI path.
PEER_SNAPSHOT_PIDS = ()
PEER_SNAPSHOT_CLI = ""
PEER_SNAPSHOT_TIMEOUT_SECONDS = 1.0
PEER_SNAPSHOT_MAX_STDOUT = 64 * 1024

# Verified by tools/bionic-socket-facade/audit.sh against the Darwin arm64
# SDK headers. These are diagnostic constants, not a portable ABI decoder.
DARWIN_MSGHDR_SIZE = 48
DARWIN_MSGHDR_CONTROL_OFFSET = 32
DARWIN_MSGHDR_CONTROLLEN_OFFSET = 40
DARWIN_MSGHDR_FLAGS_OFFSET = 44
DARWIN_CMSGHDR_SIZE = 12
DARWIN_CMSG_ALIGN = 4
DARWIN_SOL_SOCKET = 0xffff
DARWIN_SCM_RIGHTS = 1
MAX_CONTROL_BYTES = 64 * 1024
MAX_RIGHTS = 16
HOST_CALL_STACK_MAX = 32
HOST_CALLS = {}
HOST_CALLS_DROPPED = 0


class SnapshotError(RuntimeError):
    """A fail-closed diagnostic read failure."""


def _read(process, address, size, what):
    if not address or address < 0:
        raise SnapshotError("invalid %s address" % what)
    error = lldb.SBError()
    data = process.ReadMemory(address, size, error)
    if not error.Success() or data is None or len(data) != size:
        message = error.GetCString() if error.IsValid() else None
        raise SnapshotError("cannot read %s%s" % (
            what, (": " + message) if message else ""))
    return bytes(data)


def _u32(process, address, what):
    return int.from_bytes(_read(process, address, 4, what), "little")


def _u64(process, address, what):
    return int.from_bytes(_read(process, address, 8, what), "little")


def _i32(process, address, what):
    return int.from_bytes(_read(process, address, 4, what), "little",
                          signed=True)


def _module_name(module):
    spec = module.GetFileSpec()
    filename = spec.GetFilename() if spec.IsValid() else None
    if filename:
        return filename
    path = spec.GetPath() if spec.IsValid() else ""
    return os.path.basename(path)


def _symbol_address(target, module, name):
    """Return a load address for an exact symbol, or None if absent."""
    contexts = module.FindSymbols(name)
    for index in range(contexts.GetSize()):
        context = contexts.GetContextAtIndex(index)
        symbol = context.GetSymbol()
        if not symbol.IsValid():
            continue
        symbol_name = symbol.GetName()
        mangled_name = symbol.GetMangledName()
        if symbol_name != name and mangled_name != name:
            continue
        address = symbol.GetStartAddress().GetLoadAddress(target)
        if address != lldb.LLDB_INVALID_ADDRESS:
            return address
    return None


def _selected_target(debugger):
    if lldb is None:
        raise SnapshotError("LLDB Python module is unavailable")
    target = debugger.GetSelectedTarget()
    if not target or not target.IsValid():
        raise SnapshotError("no valid selected target")
    process = target.GetProcess()
    if not process or not process.IsValid():
        raise SnapshotError("no valid selected process")
    if process.GetState() != lldb.eStateStopped:
        raise SnapshotError("selected process is not stopped")
    return target, process


def _snapshot_target(target, process):
    module = next((m for m in target.module_iter()
                   if _module_name(m) == MODULE_BASENAME), None)
    if module is None:
        raise SnapshotError("native module is not loaded")

    process_symbol = _symbol_address(target, module, PROCESS_SYMBOL)
    if process_symbol is None:
        raise SnapshotError("native process symbol is unavailable")
    owner_close = _symbol_address(target, module, OWNER_CLOSE_SYMBOL)
    process_ptr = _u64(process, process_symbol, "g_process")
    if not process_ptr:
        raise SnapshotError("g_process is null")
    broker = _u64(process, process_ptr + PROCESS_BROKER_OFFSET, "broker")
    if not broker:
        raise SnapshotError("process broker is null")
    slot_bytes = _read(process, broker + BROKER_SLOTS_OFFSET,
                        SLOT_STRIDE * SLOT_COUNT, "broker slots")

    def slot_u32(index, offset):
        start = index * SLOT_STRIDE + offset
        return int.from_bytes(slot_bytes[start:start + 4], "little")

    def slot_u8(index, offset):
        return slot_bytes[index * SLOT_STRIDE + offset]

    def slot_u64(index, offset):
        start = index * SLOT_STRIDE + offset
        return int.from_bytes(slot_bytes[start:start + 8], "little")

    rows = []
    for slot in range(SLOT_COUNT):
        retired = slot_u8(slot, SLOT_RETIRED_OFFSET)
        live = slot_u8(slot, SLOT_LIVE_OFFSET)
        if retired != 0 or live != 1:
            continue
        generation = slot_u32(slot, SLOT_GENERATION_OFFSET)
        if not 1 <= generation <= GENERATION_MASK:
            continue
        description = slot_u64(slot, SLOT_DESCRIPTION_OFFSET)
        if not description:
            continue
        if _u32(process, description + DESCRIPTION_KIND_OFFSET,
                "description kind") != SOCKET_KIND:
            continue
        close_callback = _u64(process, description + DESCRIPTION_CLOSE_OFFSET,
                              "socket close callback")
        if not close_callback or (owner_close is not None and
                                  close_callback != owner_close):
            continue
        object_ptr = _u64(process, description + DESCRIPTION_OBJECT_OFFSET,
                           "socket object")
        if not object_ptr:
            continue
        refs = _u64(process, description + DESCRIPTION_REFS_OFFSET,
                    "description refs")
        active = _u64(process, description + DESCRIPTION_ACTIVE_OFFSET,
                      "description active")
        if refs == 0:
            continue
        host_fd = _i32(process, object_ptr + HOST_FD_OFFSET, "host fd")
        if host_fd < 0:
            continue
        rows.append({
            "token": TOKEN_MARKER | (generation << SLOT_BITS) | slot,
            "host_fd": host_fd,
            "description": description,
            "object": object_ptr,
            "refs": refs,
            "active": active,
        })
    return rows


def _snapshot(debugger):
    target, process = _selected_target(debugger)
    return _snapshot_target(target, process)


def _render(rows, command):
    if command.strip() == "--json":
        return json.dumps(rows, sort_keys=True, separators=(",", ":"))
    return "\n".join(
        "token=0x{token:08x} host_fd={host_fd} description=0x{description:x} "
        "object=0x{object:x} refs={refs} active={active}".format(**row)
        for row in rows
    ) or "(no live socket descriptions)"


def snapshot(debugger, command="", result=None, internal_dict=None):
    """LLDB command: snapshot the selected stopped process, without writes."""
    try:
        output = _render(_snapshot(debugger), command)
    except SnapshotError as error:
        output = "ERROR: " + str(error)
    if result is not None:
        result.PutCString(output)
        return None
    return output


TRACE_MAX_FRAMES = 16
TRACE_ADDRESS_MASK = (1 << 48) - 1
TRACE_EVENTS_MAX = 256
TRACE_EVENTS = []
TRACE_EVENTS_DROPPED = 0


def _shared_emit(event):
    """Retain an event before best-effort LLDB console output."""
    global TRACE_EVENTS_DROPPED
    if len(TRACE_EVENTS) >= TRACE_EVENTS_MAX:
        del TRACE_EVENTS[:len(TRACE_EVENTS) - TRACE_EVENTS_MAX + 1]
        TRACE_EVENTS_DROPPED += 1
    event.setdefault("trace_events_dropped", TRACE_EVENTS_DROPPED)
    TRACE_EVENTS.append(event)
    try:
        print(json.dumps(event, sort_keys=True, separators=(",", ":")))
    except Exception:
        # The in-process bounded record remains available even if LLDB's
        # console/output bridge rejects a value.
        pass


def _register_unsigned(frame, name):
    register = frame.FindRegister(name)
    if not register or not register.IsValid():
        raise SnapshotError("register %s unavailable" % name)
    return register.GetValueAsUnsigned()


def _saved_lr_chain(frame, process):
    """Read only the AArch64 x29-linked frames and saved x30 values."""
    fp = _register_unsigned(frame, "x29")
    frames = []
    for _ in range(TRACE_MAX_FRAMES):
        if not fp or fp & 7:
            break
        record = _read(process, fp, 16, "frame record")
        next_fp = int.from_bytes(record[:8], "little")
        saved_lr = int.from_bytes(record[8:16], "little")
        if saved_lr:
            frames.append(saved_lr & TRACE_ADDRESS_MASK)
        if not next_fp or next_fp <= fp:
            break
        fp = next_fp
    return frames


def shutdown_trace(frame, bp_loc, internal_dict):
    """Record a shutdown initiation and never stop execution on this BP."""
    event = {"event": "shutdown_trace", "thread": "", "token": None,
             "how": None, "rows": [], "frames": [],
             "observed_monotonic_ns": time.monotonic_ns()}
    try:
        thread = frame.GetThread()
        event["thread"] = thread.GetName() or ""
        if event["thread"] == "PerfettoTrace":
            return False
        process = thread.GetProcess()
        event["pid"] = process.GetProcessID()
        token = _register_unsigned(frame, "x0") & 0xffffffff
        event["token"] = token
        event["how"] = _register_unsigned(frame, "x1")
        event["lr"] = _register_unsigned(frame, "x30") & TRACE_ADDRESS_MASK
        target = process.GetTarget()
        if not target or not target.IsValid():
            raise SnapshotError("thread target unavailable")
        event["rows"] = [row for row in _snapshot_target(target, process)
                          if row["token"] == token]
        event["frames"] = _saved_lr_chain(frame, process)
    except Exception as error:  # Diagnostic callback must not perturb the target.
        event["error"] = str(error)[:240]
    _shared_emit(event)
    return False


_PEER_COLUMNS = (
    "pid", "fd", "soi_so", "soi_pcb", "unsi_conn_so", "unsi_conn_pcb",
    "type", "state",
)


def _peer_snapshot_rows(pid, host_fd):
    """Optionally correlate one broker fd with its reciprocal UNIX socket."""
    if not PEER_SNAPSHOT_CLI or not PEER_SNAPSHOT_PIDS:
        return [], None
    try:
        pids = tuple(str(int(value)) for value in PEER_SNAPSHOT_PIDS)
    except (TypeError, ValueError):
        return [], "invalid configured peer PID tuple"
    if any(int(value) <= 0 for value in pids):
        return [], "configured peer PID tuple contains nonpositive PID"

    try:
        child = subprocess.Popen(
            [PEER_SNAPSHOT_CLI] + list(pids), stdout=subprocess.PIPE,
            stderr=subprocess.PIPE)
        try:
            stdout, stderr = child.communicate(timeout=PEER_SNAPSHOT_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            child.kill()
            child.communicate()
            return [], "peer CLI timed out"
    except (OSError, ValueError) as error:
        return [], "peer CLI failed: %s" % error
    if len(stdout) > PEER_SNAPSHOT_MAX_STDOUT:
        return [], "peer CLI stdout exceeded bound"
    if child.returncode != 0:
        return [], "peer CLI status %d: %s" % (
            child.returncode, stderr.decode("utf-8", errors="replace")[:180])

    text = stdout.decode("utf-8", errors="surrogateescape")
    lines = text.splitlines()
    if not lines or tuple(lines[0].split("\t")) != _PEER_COLUMNS:
        return [], "peer CLI TSV header mismatch"
    rows = []
    for line in lines[1:]:
        fields = line.split("\t")
        if len(fields) != len(_PEER_COLUMNS):
            continue
        rows.append(dict(zip(_PEER_COLUMNS, fields)))

    current_pid = str(pid)
    current_fd = str(host_fd)
    current = [row for row in rows
               if row["pid"] == current_pid and row["fd"] == current_fd]
    if not current:
        return [], "host fd absent from peer snapshot"

    def pair(row, left, right):
        value = (row[left], row[right])
        return value if all(part and part != "0" for part in value) else None

    result = []
    seen = set()
    for local in current:
        local_pair = pair(local, "soi_so", "soi_pcb")
        remote_pair = pair(local, "unsi_conn_so", "unsi_conn_pcb")
        if local_pair is None:
            continue
        for candidate in rows:
            candidate_local = pair(candidate, "soi_so", "soi_pcb")
            candidate_remote = pair(candidate, "unsi_conn_so", "unsi_conn_pcb")
            if not (candidate_local == local_pair or
                    (remote_pair is not None and
                     candidate_local == remote_pair and
                     candidate_remote == local_pair)):
                continue
            identity = tuple(candidate[column] for column in _PEER_COLUMNS)
            if identity not in seen:
                seen.add(identity)
                result.append(candidate)
    return result, None


def _peer_snapshot_all():
    """Run the configured peer CLI once and retain TSV fields as strings."""
    try:
        pids = tuple(str(int(value)) for value in PEER_SNAPSHOT_PIDS)
    except (TypeError, ValueError):
        return [], "invalid configured peer PID tuple"
    if any(int(value) <= 0 for value in pids):
        return [], "configured peer PID tuple contains nonpositive PID"
    try:
        child = subprocess.Popen(
            [PEER_SNAPSHOT_CLI] + list(pids), stdout=subprocess.PIPE,
            stderr=subprocess.PIPE)
        try:
            stdout, stderr = child.communicate(timeout=PEER_SNAPSHOT_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            child.kill()
            child.communicate()
            return [], "peer CLI timed out"
    except (OSError, ValueError) as error:
        return [], "peer CLI failed: %s" % error
    if len(stdout) > PEER_SNAPSHOT_MAX_STDOUT:
        return [], "peer CLI stdout exceeded bound"
    if child.returncode != 0:
        return [], "peer CLI status %d: %s" % (
            child.returncode, stderr.decode("utf-8", errors="replace")[:180])
    lines = stdout.decode("utf-8", errors="surrogateescape").splitlines()
    if not lines or tuple(lines[0].split("\t")) != _PEER_COLUMNS:
        return [], "peer CLI TSV header mismatch"
    rows = []
    for line in lines[1:]:
        fields = line.split("\t")
        if len(fields) == len(_PEER_COLUMNS):
            rows.append(dict(zip(_PEER_COLUMNS, fields)))
    return rows, None


def _select_peer_rows(rows, pid, host_fd):
    current = [row for row in rows
               if row["pid"] == str(pid) and row["fd"] == str(host_fd)]
    if not current:
        return [], "host fd absent from peer snapshot"

    def pair(row, left, right):
        value = (row[left], row[right])
        return value if all(part and part != "0" for part in value) else None

    result = []
    seen = set()
    for local in current:
        local_pair = pair(local, "soi_so", "soi_pcb")
        remote_pair = pair(local, "unsi_conn_so", "unsi_conn_pcb")
        if local_pair is None:
            continue
        for candidate in rows:
            candidate_local = pair(candidate, "soi_so", "soi_pcb")
            candidate_remote = pair(candidate, "unsi_conn_so", "unsi_conn_pcb")
            if not (candidate_local == local_pair or
                    (remote_pair is not None and
                     candidate_local == remote_pair and
                     candidate_remote == local_pair)):
                continue
            identity = tuple(candidate[column] for column in _PEER_COLUMNS)
            if identity not in seen:
                seen.add(identity)
                result.append(candidate)
    return result, None


def _decode_rights_control(control):
    """Decode only Darwin SCM_RIGHTS, rejecting malformed/truncated data."""
    rights = []
    records = []
    offset = 0
    while offset < len(control):
        remaining = len(control) - offset
        if remaining < DARWIN_CMSGHDR_SIZE:
            raise SnapshotError("truncated cmsghdr")
        cmsg_len = int.from_bytes(control[offset:offset + 4], "little")
        level = int.from_bytes(control[offset + 4:offset + 8], "little",
                               signed=True)
        cmsg_type = int.from_bytes(control[offset + 8:offset + 12], "little",
                                   signed=True)
        if cmsg_len < DARWIN_CMSGHDR_SIZE or cmsg_len > remaining:
            raise SnapshotError("malformed cmsghdr length %d" % cmsg_len)
        payload = control[offset + DARWIN_CMSGHDR_SIZE:offset + cmsg_len]
        record = {"length": cmsg_len, "level": level, "type": cmsg_type}
        if level == DARWIN_SOL_SOCKET and cmsg_type == DARWIN_SCM_RIGHTS:
            if len(payload) % 4:
                raise SnapshotError("truncated SCM_RIGHTS payload")
            if len(payload) // 4 > MAX_RIGHTS:
                raise SnapshotError("SCM_RIGHTS count exceeds bound")
            payload_fds = [
                int.from_bytes(payload[index:index + 4], "little", signed=True)
                for index in range(0, len(payload), 4)
            ]
            rights.extend(payload_fds)
            if len(rights) > MAX_RIGHTS:
                raise SnapshotError("SCM_RIGHTS total exceeds bound")
            record["rights"] = payload_fds
        records.append(record)
        next_offset = (offset + cmsg_len + DARWIN_CMSG_ALIGN - 1) & ~(
            DARWIN_CMSG_ALIGN - 1)
        if next_offset <= offset or next_offset > len(control):
            raise SnapshotError("truncated cmsghdr alignment")
        offset = next_offset
    return rights, records


def _read_native_msghdr_header(process, message):
    if not message:
        raise SnapshotError("null native msghdr")
    header = _read(process, message, DARWIN_MSGHDR_SIZE, "native msghdr")
    control = int.from_bytes(
        header[DARWIN_MSGHDR_CONTROL_OFFSET:
               DARWIN_MSGHDR_CONTROL_OFFSET + 8], "little")
    control_length = int.from_bytes(
        header[DARWIN_MSGHDR_CONTROLLEN_OFFSET:
               DARWIN_MSGHDR_CONTROLLEN_OFFSET + 4], "little")
    flags = int.from_bytes(
        header[DARWIN_MSGHDR_FLAGS_OFFSET:
               DARWIN_MSGHDR_FLAGS_OFFSET + 4], "little", signed=True)
    return {"message": message, "control": control,
            "control_length": control_length, "flags": flags}


def _decode_native_msghdr(process, message):
    decoded = _read_native_msghdr_header(process, message)
    control = decoded["control"]
    control_length = decoded["control_length"]
    if control_length > MAX_CONTROL_BYTES:
        raise SnapshotError("control buffer exceeds bound")
    control_bytes = b""
    if control_length:
        if not control:
            raise SnapshotError("nonzero control length with null control")
        control_bytes = _read(process, control, control_length,
                              "native control buffer")
    rights, records = _decode_rights_control(control_bytes)
    decoded.update(rights=rights, control_records=records)
    return decoded


def _peer_rows_for_fds(pid, fds):
    if not PEER_SNAPSHOT_CLI or not PEER_SNAPSHOT_PIDS:
        return [], []
    rows, error = _peer_snapshot_all()
    if error:
        return [], [error]
    peers = []
    errors = []
    seen = set()
    for fd in [int(value) for value in fds]:
        if fd in seen:
            continue
        seen.add(fd)
        selected, select_error = _select_peer_rows(rows, pid, fd)
        if selected:
            peers.append({"fd": fd, "rows": selected})
        if select_error:
            errors.append("fd=%d: %s" % (fd, select_error))
    return peers, errors


def _frame_pc(frame):
    try:
        pc = frame.GetPC()
    except Exception:
        return None
    return pc & TRACE_ADDRESS_MASK if pc else None


def _register_signed(frame, name):
    value = _register_unsigned(frame, name) & ((1 << 64) - 1)
    return value - (1 << 64) if value & (1 << 63) else value


def _host_entry(frame, kind):
    """Observe a native sendmsg/recvmsg entry without changing execution."""
    event = {
        "event": "%s_entry" % kind,
        "pid": None,
        "thread": "",
        "carrier_fd": None,
        "message": None,
        "flags": None,
        "rights": [],
        "pre_control_records": [],
        "pre_peer_rows": [],
        "entry_pc": _frame_pc(frame),
    }
    errors = []
    context = None
    try:
        thread = frame.GetThread()
        process = thread.GetProcess()
        tid = thread.GetThreadID()
        event["pid"] = process.GetProcessID()
        event["thread"] = thread.GetName() or ""
        carrier_fd = _register_unsigned(frame, "x0") & 0xffffffff
        message = _register_unsigned(frame, "x1")
        flags = _register_unsigned(frame, "x2")
        event.update(carrier_fd=carrier_fd, message=message, flags=flags)
        decoded = None
        try:
            if kind == "recvmsg":
                # The receive control buffer is output storage and may contain
                # stale bytes. At entry only inspect capacity/header fields.
                decoded = _read_native_msghdr_header(process, message)
                event["control_capacity"] = decoded["control_length"]
            else:
                decoded = _decode_native_msghdr(process, message)
                event["rights"] = decoded["rights"]
                event["pre_control_records"] = decoded["control_records"]
        except Exception as error:
            errors.append("control decode: %s" % error)
        peers, peer_errors = _peer_rows_for_fds(
            event["pid"], [carrier_fd] + event["rights"])
        event["pre_peer_rows"] = peers
        errors.extend(peer_errors)
        context = dict(kind=kind, pid=event["pid"], tid=tid,
                       carrier_fd=carrier_fd, message=message,
                       entry_pc=event["entry_pc"], entry=decoded)
        stack = HOST_CALLS.setdefault(tid, [])
        global HOST_CALLS_DROPPED
        if len(stack) >= HOST_CALL_STACK_MAX:
            del stack[0]
            HOST_CALLS_DROPPED += 1
        stack.append(context)
    except Exception as error:
        errors.append(str(error))
    if errors:
        event["errors"] = [str(error)[:240] for error in errors[:4]]
    _shared_emit(event)
    return False


def _host_return(frame, kind):
    """Observe a statically-installed return breakpoint for one host call."""
    event = {"event": "%s_return" % kind, "pid": None,
             "thread": "", "return_pc": _frame_pc(frame), "rights": [],
             "post_control_records": [], "post_peer_rows": []}
    errors = []
    context = None
    try:
        thread = frame.GetThread()
        process = thread.GetProcess()
        tid = thread.GetThreadID()
        event["pid"] = process.GetProcessID()
        event["thread"] = thread.GetName() or ""
        stack = HOST_CALLS.get(tid, [])
        if not stack or stack[-1]["kind"] != kind:
            event["mismatched_return"] = True
        else:
            context = stack.pop()
        if context is None:
            event["pending_dropped"] = HOST_CALLS_DROPPED
        else:
            event.update(carrier_fd=context["carrier_fd"],
                         message=context["message"],
                         entry_pc=context["entry_pc"],
                         result=_register_signed(frame, "x0"))
            if event["result"] < 0 and kind == "recvmsg":
                event["recv_failed"] = True
                event["pending_dropped"] = HOST_CALLS_DROPPED
                if not stack:
                    HOST_CALLS.pop(tid, None)
                _shared_emit(event)
                return False
            decoded = None
            try:
                decoded = _decode_native_msghdr(process, context["message"])
                event["rights"] = decoded["rights"]
                event["post_control_records"] = decoded["control_records"]
            except Exception as error:
                errors.append("control decode: %s" % error)
            peers, peer_errors = _peer_rows_for_fds(
                event["pid"], [context["carrier_fd"]] + event["rights"])
            event["post_peer_rows"] = peers
            errors.extend(peer_errors)
        if not stack:
            HOST_CALLS.pop(tid, None)
    except Exception as error:
        errors.append(str(error))
    if errors:
        event["errors"] = [str(error)[:240] for error in errors[:4]]
    _shared_emit(event)
    return False


def host_sendmsg_trace(frame, bp_loc, internal_dict):
    return _host_entry(frame, "sendmsg")


def host_recvmsg_trace(frame, bp_loc, internal_dict):
    return _host_entry(frame, "recvmsg")


def host_sendmsg_return_trace(frame, bp_loc, internal_dict):
    return _host_return(frame, "sendmsg")


def host_recvmsg_return_trace(frame, bp_loc, internal_dict):
    return _host_return(frame, "recvmsg")


def close_trace(frame, bp_loc, internal_dict):
    """Record a socket close entry without stopping or changing the target."""
    try:
        thread = frame.GetThread()
        thread_name = thread.GetName() or ""
        if "Perfetto" in thread_name:
            return False
        process = thread.GetProcess()
        token = _register_unsigned(frame, "x0") & 0xffffffff
        target = process.GetTarget()
        if not target or not target.IsValid():
            raise SnapshotError("thread target unavailable")
        matching = [row for row in _snapshot_target(target, process)
                    if row["token"] == token]
        if not matching:  # Non-socket or already-recycled token.
            return False
        lr = _register_unsigned(frame, "x30") & TRACE_ADDRESS_MASK
        x29 = _register_unsigned(frame, "x29")
        saved_lr = _saved_lr_chain(frame, process)
        pid = process.GetProcessID()
        peer_rows = []
        errors = []
        for row in matching:
            peers, error = _peer_snapshot_rows(pid, row["host_fd"])
            peer_rows.extend(peers)
            if error:
                errors.append(error)
        event = {
            "event": "close_trace",
            "pid": pid,
            "thread": thread_name,
            "token": token,
            "rows": matching,
            "lr": lr,
            "x29": x29,
            "savedLR": saved_lr,
            "observed_monotonic_ns": time.monotonic_ns(),
        }
        if peer_rows:
            event["peer_rows"] = peer_rows
        if errors:
            event["errors"] = errors[:4]
        _shared_emit(event)
    except Exception as error:  # Never perturb the breakpointed target.
        _shared_emit({"event": "close_trace", "error": str(error)[:240]})
    return False


def host_shutdown_trace(frame, bp_loc, internal_dict):
    """Record host-fd shutdowns, including descriptors outside the broker."""
    event = {
        "event": "host_shutdown_trace",
        "pid": None,
        "thread": "",
        "host_fd": None,
        "how": None,
        "lr": None,
        "savedLR": [],
        "observed_monotonic_ns": time.monotonic_ns(),
        "rows": [],
    }
    errors = []
    try:
        thread = frame.GetThread()
        event["thread"] = thread.GetName() or ""
        process = thread.GetProcess()
        event["pid"] = process.GetProcessID()
        event["host_fd"] = _register_unsigned(frame, "x0") & 0xffffffff
        event["how"] = _register_unsigned(frame, "x1")
        event["lr"] = _register_unsigned(frame, "x30") & TRACE_ADDRESS_MASK
        try:
            event["savedLR"] = _saved_lr_chain(frame, process)
        except SnapshotError as error:
            errors.append("frame chain: %s" % error)

        # The host close path can be for a raw Binder/facade fd, so a missing
        # broker or socket row is recorded as an empty match, not a failure.
        try:
            target = process.GetTarget()
            if not target or not target.IsValid():
                raise SnapshotError("thread target unavailable")
            event["rows"] = [
                row for row in _snapshot_target(target, process)
                if row["host_fd"] == event["host_fd"]
            ]
        except Exception as error:
            errors.append("native snapshot: %s" % error)

        peers, peer_error = _peer_snapshot_rows(event["pid"], event["host_fd"])
        if peers:
            event["peer_rows"] = peers
        if peer_error:
            errors.append(peer_error)
    except Exception as error:
        errors.append(str(error))
    if errors:
        event["errors"] = [str(error)[:240] for error in errors[:4]]
    _shared_emit(event)
    return False


def __lldb_init_module(debugger, internal_dict):
    debugger.HandleCommand(
        "command script add -f %s.snapshot socket-broker-snapshot" % __name__)
    print("socket-broker-snapshot: read-only selected-stopped-process snapshot")

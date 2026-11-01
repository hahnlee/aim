"""Offline schema-driven wire decoder for the WM dump regression (no generated tags).

Read message and field declarations from the complete hash-pinned AOSP schemas.
Unknown fields, duplicate singular fields and wire-type mismatches fail this test.
"""
import hashlib
import pathlib
import re
import struct
import sys

root, lock, data = map(pathlib.Path, sys.argv[1:])
messages = {}
for project, path, digest in re.findall(r'"(platform/[^|]+)\|([^|]+\.proto)\|([0-9a-f]{64})"', lock.read_text()):
    raw = (root / project.removeprefix('platform/') / path).read_bytes()
    assert hashlib.sha256(raw).hexdigest() == digest, path
    text = re.sub(r'/\*.*?\*/|//[^\n]*', '', raw.decode(), flags=re.S)
    for match in re.finditer(r'\bmessage\s+(\w+)\s*\{', text):
        start, depth, end = match.end(), 1, match.end()
        while depth:
            depth += (text[end] == '{') - (text[end] == '}')
            end += 1
        fields = {}
        for repeated, kind, name, number in re.findall(r'\b(optional|repeated|required)\s+([.\w]+)\s+(\w+)\s*=\s*(\d+)', text[start:end - 1]):
            fields[int(number)] = (repeated == 'repeated', kind.rsplit('.', 1)[-1], name)
        assert match[1] not in messages, match[1]
        messages[match[1]] = fields


def decode(kind, raw):
    fields, out, pos = messages[kind], {}, 0

    def varint():
        nonlocal pos
        value, shift = 0, 0
        while True:
            b = raw[pos]
            pos += 1
            value |= (b & 127) << shift
            if b < 128:
                return value
            shift += 7
            assert shift < 70

    while pos < len(raw):
        key = varint()
        repeated, typ, name = fields[key >> 3]
        wire = key & 7
        if typ in messages or typ in ('string', 'bytes'):
            assert wire == 2, (kind, name, wire)
            length = varint()
            end = pos + length
            assert end <= len(raw)
            value, pos = raw[pos:end], end
            value = decode(typ, value) if typ in messages else value.decode() if typ == 'string' else value
        elif typ == 'float':
            assert wire == 5
            value = struct.unpack_from('<f', raw, pos)[0]
            pos += 4
        else:
            assert wire == 0, (kind, name, wire)
            value = varint()
            if typ in ('int32', 'uint32'):
                value &= 0xffffffff
                if typ == 'int32' and value >= 2**31:
                    value -= 2**32
        if repeated:
            out.setdefault(name, []).append(value)
        else:
            assert name not in out, (kind, name)
            out[name] = value
    assert pos == len(raw)
    return out


state = decode('WindowManagerServiceDumpProto', data.read_bytes())
assert state['policy'] == {}  # Presence required by the legacy CTS decoder.
assert state['window_frames_valid'] == 1
assert state['focused_app'] == 'p/.A8'
assert state['focused_window']['title'] == 'popup'
assert 'input_method_window' not in state
root = state['root_window_container']['window_container']
display = root['children'][0]['display_content']
assert display['focused_root_task_id'] == 8
assert display['resumed_activity']['title'] == 'p/.A8'
assert 'input_method_input_target_identifier' not in display
area = display['root_display_area']['window_container']['children'][0]['display_area']
assert area['is_task_display_area'] == 1 and area['feature_id'] == 1
for index, child in enumerate(area['window_container']['children']):
    task = child['task']
    ident = 7 + index
    assert task['id'] == task['root_task_id'] == ident
    assert task['resumed_activity']['title'] == f'p/.A{ident}'
    wc = task['task_fragment']['window_container']
    config = wc['configuration_container']['full_configuration']
    window = config['window_configuration']
    assert window['windowing_mode'] == 5
    assert window['bounds'] == dict(left=20 if ident == 7 else 600, top=50, right=420 if ident == 7 else 1000, bottom=850)
    assert config['screen_width_dp'] == 400 and config['screen_height_dp'] == 800
    assert len(wc['children']) == (2 if ident == 8 else 1)
    activity = wc['children'][0]['activity']
    assert activity['state'] == 'RESUMED' and activity['visible'] == 1
    win = activity['window_token']['window_container']['children'][0]['window']
    assert win['window_frames']['frame'] == window['bounds']
    assert win['animator']['surface']['shown'] == 1
    assert win['attributes']['type'] == 1
    if ident == 8:
        assert wc['children'][1]['window']['stack_id'] == 8
        assert len(win['window_container']['children']) == 1
        popup = win['window_container']['children'][0]['window']
        assert 'children' not in popup['window_container']
print('Pinned WM schema: multi-task hierarchy, configuration, focus, resumed and subwindows verified')

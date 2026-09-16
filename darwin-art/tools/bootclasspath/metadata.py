"""Read Android classpaths.proto without requiring generated host protobuf code."""
from pathlib import PurePosixPath


def varint(data, at):
    value = 0
    for shift in range(0, 70, 7):
        if at >= len(data):
            raise ValueError("truncated varint")
        byte = data[at]
        at += 1
        if shift == 63 and byte > 1:
            raise ValueError("varint overflow")
        value |= (byte & 127) << shift
        if byte < 128:
            return value, at
    raise ValueError("varint overflow")


def fields(data):
    at = 0
    while at < len(data):
        tag, at = varint(data, at)
        number, wire = tag >> 3, tag & 7
        if not number:
            raise ValueError("zero field number")
        if wire == 0:
            value, at = varint(data, at)
        elif wire in (1, 2, 5):
            size = 8 if wire == 1 else 4
            if wire == 2:
                size, at = varint(data, at)
            if size > len(data) - at:
                raise ValueError("truncated field")
            value = data[at:at + size]
            at += size
        else:
            raise ValueError("unsupported protobuf wire type")
        yield number, wire, value


def boot_jars(data, sdk=36):
    result = []
    for number, wire, value in fields(data):
        if number != 1:
            continue
        if wire != 2:
            raise ValueError("invalid classpath entry")
        entry = {}
        for key, kind, item in fields(value):
            if key in (1, 2, 3, 4):
                if key in entry or kind != (0 if key == 2 else 2):
                    raise ValueError("duplicate or mistyped classpath field")
                entry[key] = item
        if entry.get(2) != 1:  # BOOTCLASSPATH, not DEX2OATBOOTCLASSPATH
            continue
        path = entry[1].decode("utf-8")
        pure = PurePosixPath(path)
        if not pure.is_absolute() or str(pure) != path or ".." in pure.parts:
            raise ValueError("unsafe classpath path")
        if not path.endswith(".jar"):
            raise ValueError("classpath entry is not a JAR")
        bounds = [entry.get(key, b"").decode("ascii") for key in (3, 4)]
        if any(bound and not bound.isdecimal() for bound in bounds):
            raise ValueError("unresolved SDK codename in release image")
        if bounds[0] and sdk < int(bounds[0]) or bounds[1] and sdk > int(bounds[1]):
            continue
        if path in result:
            raise ValueError("duplicate boot JAR")
        result.append(path)
    return result

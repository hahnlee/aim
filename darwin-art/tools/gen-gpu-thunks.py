#!/usr/bin/env python3
"""Generate the EGL/GLES host-call thunks from the Khronos XML registry.

The guest GLES driver (`hal/gles`, `/vendor/lib64/egl/libGLES_darwin.so`)
exports every EGL and GLES entry point the host's ANGLE implements. Each one
packs its arguments into the host's register image and makes one host call
to the host module `gpu` (`crates/darwin-host-gpu`), which calls ANGLE's
entry point with them (docs/gles-driver.md).

A command is forwarded when the registry defines it (gl.xml, egl.xml and
ANGLE's own extension XML) and the pinned ANGLE build exports it. Its
signature comes from the registry, and the output is checked in, so builds
need neither the registry nor ANGLE.

Usage: tools/gen-gpu-thunks.py [--angle DIR]
  DIR: ANGLE's source tree with its build in out/DarwinArtRelease
       (default _build/angle-source).
"""

import argparse
import hashlib
import os
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUEST_OUT = os.path.join(ROOT, "hal/gles/src/thunks.rs")
HOST_OUT = os.path.join(ROOT, "crates/darwin-host-gpu/src/table.rs")

# Scalar types: Rust type, register class ("x" or "d") and size in bytes.
SCALARS = {
    "GLenum": ("u32", "x", 4), "GLboolean": ("u8", "x", 1),
    "GLbitfield": ("u32", "x", 4), "GLbyte": ("i8", "x", 1),
    "GLubyte": ("u8", "x", 1), "GLshort": ("i16", "x", 2),
    "GLushort": ("u16", "x", 2), "GLint": ("i32", "x", 4),
    "GLuint": ("u32", "x", 4), "GLfixed": ("i32", "x", 4),
    "GLclampx": ("i32", "x", 4), "GLsizei": ("i32", "x", 4),
    "GLfloat": ("f32", "d", 4), "GLclampf": ("f32", "d", 4),
    "GLintptr": ("isize", "x", 8), "GLsizeiptr": ("isize", "x", 8),
    "GLint64": ("i64", "x", 8), "GLint64EXT": ("i64", "x", 8),
    "GLuint64": ("u64", "x", 8), "GLuint64EXT": ("u64", "x", 8),
    "GLchar": ("i8", "x", 1), "GLhalf": ("u16", "x", 2),
    "EGLBoolean": ("u32", "x", 4), "EGLint": ("i32", "x", 4),
    "EGLenum": ("u32", "x", 4), "EGLAttrib": ("isize", "x", 8),
    "EGLAttribKHR": ("isize", "x", 8), "EGLTime": ("u64", "x", 8),
    "EGLTimeKHR": ("u64", "x", 8), "EGLTimeNV": ("u64", "x", 8),
    "EGLuint64KHR": ("u64", "x", 8), "EGLuint64NV": ("u64", "x", 8),
    "EGLnsecsANDROID": ("i64", "x", 8),
    "EGLNativeFileDescriptorKHR": ("i32", "x", 4),
    "EGLsizeiANDROID": ("isize", "x", 8),
}
# Opaque handles and pointers (one x register).
HANDLES = {
    "GLsync", "GLeglImageOES", "GLeglClientBufferEXT", "EGLDisplay",
    "EGLConfig", "EGLSurface", "EGLContext", "EGLClientBuffer", "EGLImage",
    "EGLImageKHR", "EGLSync", "EGLSyncKHR", "EGLSyncNV", "EGLStreamKHR",
    "EGLDeviceEXT", "EGLOutputLayerEXT", "EGLOutputPortEXT", "EGLLabelKHR",
    "EGLObjectKHR", "EGLNativeDisplayType", "EGLNativeWindowType",
    "EGLNativePixmapType",
}
# Pointed-to types that are not scalars (only ever behind a pointer).
POINTEES = {"void", "wl_display", "wl_resource", "wl_buffer",
            "EGLClientPixmapHI", "AHardwareBuffer", "_cl_context", "_cl_event"}
# Function-pointer types: host code never calls guest code, so commands that
# take one are not forwarded (hand-written guest code may stand in).
CALLBACKS = {"GLDEBUGPROC", "GLDEBUGPROCKHR", "EGLDEBUGPROCKHR",
             "EGLSetBlobFuncANDROID", "EGLGetBlobFuncANDROID",
             "__eglMustCastToProperFunctionPointerType", "GLVULKANPROCNV",
             "GLSETBLOBPROCANGLE", "GLGETBLOBPROCANGLE"}

# Entry points the guest implements itself (hal/gles/src/egl.rs and gl.rs).
# Their host thunk, if any, is still generated for that code to use.
GUEST = {
    # EGL on the Android platform: displays, window surfaces over
    # ANativeWindow, Android native buffers, the Android config attributes.
    "eglGetDisplay": "egl", "eglGetPlatformDisplay": "egl",
    "eglGetPlatformDisplayEXT": "egl",
    "eglQueryString": "egl", "eglChooseConfig": "egl",
    "eglGetConfigAttrib": "egl", "eglCreateWindowSurface": "egl",
    "eglCreatePlatformWindowSurface": "egl",
    "eglCreatePlatformWindowSurfaceEXT": "egl", "eglDestroySurface": "egl",
    "eglMakeCurrent": "egl", "eglGetCurrentSurface": "egl",
    "eglSwapBuffers": "egl", "eglSwapBuffersWithDamageKHR": "egl",
    "eglSwapBuffersWithDamageEXT": "egl", "eglSetDamageRegionKHR": "egl",
    "eglSwapInterval": "egl", "eglQuerySurface": "egl",
    "eglSurfaceAttrib": "egl", "eglCreateImageKHR": "egl",
    "eglCreateImage": "egl", "eglDestroyImageKHR": "egl",
    "eglDestroyImage": "egl", "eglGetError": "egl",
    "eglPresentationTimeANDROID": "egl", "eglGetProcAddress": "egl",
    # Callbacks the host never makes.
    "eglSetBlobCacheFuncsANDROID": "egl", "eglDebugMessageControlKHR": "egl",
    "glDebugMessageCallback": "gl", "glDebugMessageCallbackKHR": "gl",
    # GL_OES_EGL_image_external over 2D textures (ANGLE's Metal backend
    # lacks it).
    "glGetString": "external", "glGetStringi": "external",
    "glGetIntegerv": "external", "glBindTexture": "external",
    "glEGLImageTargetTexture2DOES": "external", "glTexParameteri": "external",
    "glTexParameterf": "external", "glTexParameteriv": "external",
    "glTexParameterfv": "external", "glGetTexParameteriv": "external",
    "glGetTexParameterfv": "external", "glShaderSource": "external",
    "glLinkProgram": "external", "glUniform1i": "external",
    "glUniform1iv": "external",
}


def text_of(elem):
    return "".join(elem.itertext())


class Param:
    def __init__(self, elem):
        ptype = elem.find("ptype")
        text = text_of(elem)
        if ptype is not None:
            self.base = ptype.text
        else:
            self.base = "char" if "char" in text.replace(elem.find("name").text, "") else "void"
        self.name = elem.find("name").text
        self.pointers = text.count("*")
        self.const = text.strip().startswith("const")

    def rust(self):
        if self.base in CALLBACKS:
            return None
        if self.pointers:
            if self.base not in set(SCALARS) | HANDLES | POINTEES | {"char"}:
                raise SystemExit(f"unknown type {self.base}")
            inner = {"char": "c_char"}.get(self.base, "c_void" if self.base in POINTEES else self.base)
            kind = "*const " if self.const else "*mut "
            # Pointers to pointers: the inner level is `*mut`/`*const`.
            return kind + "*mut " * (self.pointers - 1) + inner
        if self.base in SCALARS or self.base in HANDLES:
            return self.base
        if self.base == "void":
            return ""
        raise SystemExit(f"unknown type {self.base}")

    def reg(self):
        """(class, size) of the value in the host call."""
        if self.pointers or self.base in HANDLES:
            return ("x", 8)
        _, cls, size = SCALARS[self.base]
        return (cls, size)


def rust_name(name):
    keywords = {"type", "ref", "in", "fn", "match", "loop", "mod", "priv", "self", "impl"}
    return f"r#{name}" if name in keywords else name


class Command:
    def __init__(self, elem, lib):
        proto = elem.find("proto")
        self.name = proto.find("name").text
        self.ret = Param(proto)
        self.params = [Param(p) for p in elem.findall("param")]
        self.lib = lib

    def forwardable(self):
        types = [self.ret.base] + [p.base for p in self.params]
        return not any(t in CALLBACKS for t in types)

    def layout(self):
        """Darwin arm64 argument placement: (nx, nd, stack words, per-param
        (kind, index or byte offset))."""
        nx = nd = 0
        stack = 0
        places = []
        for p in self.params:
            cls, size = p.reg()
            if cls == "x" and nx < 8:
                places.append(("x", nx)); nx += 1
            elif cls == "d" and nd < 8:
                places.append(("d", nd)); nd += 1
            else:
                # Apple's arm64 ABI packs stack arguments at their natural
                # alignment rather than in 8-byte slots.
                stack = (stack + size - 1) // size * size
                places.append(("s", stack)); stack += size
        return nx, nd, (stack + 7) // 8, places


def load(path, lib, commands):
    for elem in ET.parse(path).getroot().iter("command"):
        if elem.find("proto") is None:
            continue
        c = Command(elem, lib)
        commands.setdefault(c.name, c)


def exports(dylib):
    out = subprocess.run(["nm", "-gU", dylib], check=True, capture_output=True, text=True).stdout
    return {line.split()[-1][1:] for line in out.splitlines() if line.split()}


def ret_rust(c):
    r = c.ret.rust()
    return f" -> {r}" if r else ""


def pack(c, arg_prefix=""):
    """Rust statements filling `regs`, the host's register image."""
    nx, nd, ns, places = c.layout()
    lines = []
    total = nx + nd + ns
    if total == 0:
        return "let mut regs = [0u64; 0];", 0
    lines.append(f"let mut regs = [0u64; {total}];")
    for p, (kind, at) in zip(c.params, places):
        name = arg_prefix + rust_name(p.name)
        cls, size = p.reg()
        if cls == "d":
            bits = f"{name}.to_bits() as u64"
        elif p.pointers or p.base in HANDLES:
            bits = f"{name} as u64"
        else:
            # Darwin callers extend narrow integers to 32 bits; sign
            # extension to 64 covers that.
            bits = f"{name} as u64"
        if kind == "x":
            lines.append(f"regs[{at}] = {bits};")
        elif kind == "d":
            lines.append(f"regs[{nx + at}] = {bits};")
        else:
            word, shift = nx + nd + at // 8, (at % 8) * 8
            mask = "" if size == 8 else f" & {(1 << (size * 8)) - 1:#x}"
            shifted = f"({bits}{mask}) << {shift}" if shift else f"{bits}{mask}"
            lines.append(f"regs[{word}] |= {shifted};")
    return " ".join(lines), total


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--angle", default=os.path.join(ROOT, "_build/angle-source"))
    args = ap.parse_args()
    angle = args.angle
    out_dir = os.path.join(angle, "out/DarwinArtRelease")
    registry = [
        (os.path.join(angle, "third_party/EGL-Registry/src/api/egl.xml"), "egl"),
        (os.path.join(angle, "scripts/egl_angle_ext.xml"), "egl"),
        (os.path.join(angle, "third_party/OpenGL-Registry/src/xml/gl.xml"), "gl"),
        (os.path.join(angle, "scripts/gl_angle_ext.xml"), "gl"),
    ]
    commands = {}
    for path, lib in registry:
        load(path, lib, commands)
    have = {"egl": exports(os.path.join(out_dir, "libEGL.dylib")),
            "gl": exports(os.path.join(out_dir, "libGLESv2.dylib"))}
    head = subprocess.run(["git", "-C", angle, "rev-parse", "HEAD"], capture_output=True,
                          text=True).stdout.strip() or "unknown"

    table = sorted((c for c in commands.values()
                    if c.name in have[c.lib] and c.forwardable()),
                   key=lambda c: (c.lib, c.name))
    digest = hashlib.sha256()
    for c in table:
        nx, nd, ns, _ = c.layout()
        digest.update(f"{c.name}:{c.lib}:{nx}:{nd}:{ns};".encode())
    table_hash = int.from_bytes(digest.digest()[:8], "little")
    source = f"the Khronos registry (gl.xml, egl.xml) and ANGLE {head}"

    host = [
        f"// @generated by tools/gen-gpu-thunks.py from {source}. Do not edit.",
        "",
        "use crate::{Entry, Lib};",
        "",
        "/// Identifies this table; the guest driver must present the same.",
        f"pub const TABLE_HASH: u64 = {table_hash:#018x};",
        "",
        "/// Forwarded entry points, indexed by host-call function id",
        "/// minus `gpu::FN_TABLE_BASE`.",
        "pub static ENTRIES: [Entry; %d] = [" % len(table),
    ]
    for c in table:
        nx, nd, ns, _ = c.layout()
        lib = "Egl" if c.lib == "egl" else "Gles"
        host.append(f'    Entry {{ name: c"{c.name}", lib: Lib::{lib}, nx: {nx}, nd: {nd}, ns: {ns} }},')
    host.append("];")

    guest = [
        f"// @generated by tools/gen-gpu-thunks.py from {source}. Do not edit.",
        "//",
        "// `host::*` makes the host call; the exported symbols call it, except",
        "// for the entry points implemented in `crate::egl` and `crate::gl`.",
        "#![allow(non_snake_case, clippy::missing_safety_doc, clippy::too_many_arguments)]",
        "#![allow(clippy::unnecessary_cast, clippy::identity_op)]",
        "",
        "use core::ffi::{c_char, c_void};",
        "",
        "use crate::types::*;",
        "",
        f"pub const TABLE_HASH: u64 = {table_hash:#018x};",
        f"pub const TABLE_LEN: usize = {len(table)};",
        "",
        "pub mod host {",
        "    use super::*;",
        "    use crate::hostcall::call;",
    ]
    exported = []
    for i, c in enumerate(table):
        params = ", ".join(f"{rust_name(p.name)}: {p.rust()}" for p in c.params)
        body, n = pack(c)
        ret = c.ret.rust()
        call = f"call({i}, regs.as_mut_ptr(), {n})"
        if ret:
            if ret.startswith("*"):
                tail = f"{call} as usize as {ret}"
            else:
                tail = f"{call} as {ret}"
        else:
            tail = f"{call};"
        guest.append("    #[inline]")
        guest.append(f"    pub unsafe fn {c.name}({params}){ret_rust(c)} {{")
        guest.append(f"        {body}")
        guest.append(f"        unsafe {{ {tail} }}")
        guest.append("    }")
        if c.name not in GUEST:
            exported.append(c)
    guest.append("}")
    guest.append("")
    for c in exported:
        params = ", ".join(f"{rust_name(p.name)}: {p.rust()}" for p in c.params)
        names = ", ".join(rust_name(p.name) for p in c.params)
        guest.append("#[unsafe(no_mangle)]")
        guest.append(f"pub unsafe extern \"C\" fn {c.name}({params}){ret_rust(c)} {{")
        guest.append(f"    unsafe {{ host::{c.name}({names}) }}")
        guest.append("}")
    # eglGetProcAddress: every exported entry point, with its host id (to
    # check that the host resolved it) or -1 for guest code.
    procs = {c.name: (c.name, i) for i, c in enumerate(table) if c.name not in GUEST}
    for name, module in GUEST.items():
        procs[name] = (f"crate::{module}::{name}", -1)
    guest.append("")
    guest.append("/// Every exported entry point, sorted by name.")
    guest.append("pub static PROCS: [Proc; %d] = [" % len(procs))
    for name in sorted(procs):
        path, hid = procs[name]
        guest.append(f'    Proc {{ name: "{name}", addr: {path} as *const c_void, id: {hid} }},')
    guest.append("];")
    guest.append("")

    for path, lines in ((HOST_OUT, host), (GUEST_OUT, guest)):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write("\n".join(lines) + "\n")
    print(f"{len(table)} entry points ({sum(c.lib == 'egl' for c in table)} EGL), "
          f"table hash {table_hash:#018x}")


if __name__ == "__main__":
    sys.exit(main())

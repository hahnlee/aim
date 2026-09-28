#!/usr/bin/env python3
"""Generate the Vulkan host-call thunks from the Khronos registry (vk.xml).

The guest Vulkan driver (`hal/vulkan`, `/vendor/lib64/hw/vulkan.aim.so`)
hands the original libvulkan a thunk for every Vulkan command the host's
MoltenVK implements. Each one packs its arguments into the host's register
image and makes one host call to the host module `vulkan`
(`crates/aim-host-vulkan`), which calls MoltenVK's entry point with them
(docs/vulkan-driver.md).

A command is forwarded when the registry defines it for the `vulkan` API,
the pinned MoltenVK release exports it (under its name or an alias), and it
belongs to a core version or to an extension the driver does not withhold.
The output is checked in, so builds need neither the registry nor MoltenVK.

Usage: tools/gen-vulkan-thunks.py [--moltenvk DIR]
  DIR: the `moltenvk` node's output with libMoltenVK.dylib and vk.xml
       (default target/aim/moltenvk).
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
GUEST_OUT = os.path.join(ROOT, "hal/vulkan/src/thunks.rs")
HOST_OUT = os.path.join(ROOT, "crates/aim-host-vulkan/src/table.rs")

# Extensions of MoltenVK the driver does not offer, with their commands.
WITHHELD_INSTANCE = [
    # Window-system integration is the original loader's (over
    # VK_ANDROID_native_buffer); MoltenVK's is for CAMetalLayer.
    "VK_KHR_surface", "VK_KHR_get_surface_capabilities2",
    "VK_EXT_surface_maintenance1", "VK_KHR_surface_maintenance1",
    "VK_EXT_swapchain_colorspace", "VK_EXT_metal_surface",
    "VK_MVK_macos_surface", "VK_MVK_ios_surface", "VK_EXT_headless_surface",
    # The loader enumerates our device as a conformant one.
    "VK_KHR_portability_enumeration",
    # Callbacks into guest code, which host code never makes.
    "VK_EXT_debug_report", "VK_EXT_debug_utils",
    # MoltenVK's own configuration, which is ours to set.
    "VK_EXT_layer_settings", "VK_MVK_moltenvk",
]
WITHHELD_DEVICE = [
    # Window-system integration (see above).
    "VK_KHR_swapchain", "VK_KHR_swapchain_mutable_format",
    "VK_EXT_swapchain_maintenance1", "VK_KHR_swapchain_maintenance1",
    "VK_KHR_present_id", "VK_KHR_present_id2", "VK_KHR_present_wait",
    "VK_KHR_present_wait2", "VK_KHR_incremental_present",
    "VK_GOOGLE_display_timing", "VK_EXT_hdr_metadata",
    # Metal objects, which guest code cannot hold.
    "VK_EXT_metal_objects", "VK_EXT_external_memory_metal",
]

# Entry points the guest implements itself, by module of hal/vulkan/src.
# Their host thunk, if any, is still generated for that code to use.
# Names are canonical; aliases get the same entry point.
GUEST = {
    # The Android driver interface: proc addresses, extension lists,
    # instance and device creation, queue families and the queues the
    # driver also submits to.
    "vkGetInstanceProcAddr": "driver", "vkGetDeviceProcAddr": "driver",
    "vkEnumerateInstanceExtensionProperties": "driver",
    "vkEnumerateDeviceExtensionProperties": "driver",
    "vkCreateInstance": "driver", "vkCreateDevice": "driver",
    "vkDestroyDevice": "driver", "vkGetDeviceQueue": "driver",
    "vkGetDeviceQueue2": "driver", "vkQueueSubmit": "driver",
    "vkGetPhysicalDeviceQueueFamilyProperties": "driver",
    "vkGetPhysicalDeviceQueueFamilyProperties2": "driver",
    "vkQueueSubmit2": "driver",
    "vkQueueWaitIdle": "driver", "vkQueueBindSparse": "driver",
    # VK_ANDROID_native_buffer: swapchain images over gralloc buffers.
    "vkGetSwapchainGrallocUsage2ANDROID": "native_buffer",
    "vkAcquireImageANDROID": "native_buffer",
    "vkQueueSignalReleaseImageANDROID": "native_buffer",
    "vkCreateImage": "native_buffer", "vkDestroyImage": "native_buffer",
    # VK_ANDROID_external_memory_android_hardware_buffer.
    "vkGetAndroidHardwareBufferPropertiesANDROID": "ahb",
    "vkGetMemoryAndroidHardwareBufferANDROID": "ahb",
    "vkAllocateMemory": "ahb", "vkFreeMemory": "ahb", "vkCreateBuffer": "ahb",
    "vkGetPhysicalDeviceImageFormatProperties2": "ahb",
    "vkGetPhysicalDeviceExternalBufferProperties": "ahb",
    # VK_KHR_external_semaphore_fd with sync fds.
    "vkCreateSemaphore": "sync_fd", "vkGetSemaphoreFdKHR": "sync_fd",
    "vkImportSemaphoreFdKHR": "sync_fd",
    "vkGetPhysicalDeviceExternalSemaphoreProperties": "sync_fd",
}

# Scalars: Rust type, register class ("x" or "d") and size in bytes.
SCALARS = {
    "uint8_t": ("u8", "x", 1), "uint16_t": ("u16", "x", 2),
    "uint32_t": ("u32", "x", 4), "int32_t": ("i32", "x", 4),
    "uint64_t": ("u64", "x", 8), "int64_t": ("i64", "x", 8),
    "size_t": ("usize", "x", 8), "int": ("i32", "x", 4),
    "float": ("f32", "d", 4), "VkBool32": ("u32", "x", 4),
    "VkDeviceSize": ("u64", "x", 8), "VkDeviceAddress": ("u64", "x", 8),
    "VkFlags": ("u32", "x", 4), "VkFlags64": ("u64", "x", 8),
    "VkSampleMask": ("u32", "x", 4),
}


def text_of(elem):
    return "".join(elem.itertext())


def for_vulkan(elem):
    api = elem.get("api")
    return api is None or "vulkan" in api.split(",")


class Registry:
    def __init__(self, path):
        self.root = ET.parse(path).getroot()
        self.types = {}
        for t in self.root.find("types"):
            if not for_vulkan(t):
                continue
            name = t.get("name") or (t.find("name").text if t.find("name") is not None else None)
            if name:
                self.types[name] = t
        self.commands, self.aliases = {}, {}
        for c in self.root.find("commands"):
            if not for_vulkan(c):
                continue
            if c.get("alias"):
                self.aliases[c.get("name")] = c.get("alias")
            else:
                self.commands[c.find("proto/name").text] = c
        # Commands required by a core version or by an offered extension.
        withheld = set(WITHHELD_INSTANCE) | set(WITHHELD_DEVICE)
        self.offered = set()
        for feature in self.root.iter("feature"):
            if for_vulkan(feature):
                self.offered |= {c.get("name") for c in feature.iter("command")}
        for ext in self.root.find("extensions"):
            if ext.get("name") in withheld or "vulkan" not in (ext.get("supported") or "").split(","):
                continue
            self.offered |= {c.get("name") for c in ext.iter("command")}
        self.copyright = ""
        comment = self.root.find("comment")
        if comment is not None:
            m = re.search(r"Copyright[^\n]*", comment.text)
            self.copyright = m.group(0).strip() if m else ""

    def resolve(self, name):
        t = self.types.get(name)
        while t is not None and t.get("alias"):
            name = t.get("alias")
            t = self.types.get(name)
        return name, t

    def kind(self, name):
        """Rust type, register class and size of a value of type `name`."""
        if name in SCALARS:
            return SCALARS[name]
        name, t = self.resolve(name)
        if name in SCALARS:
            return SCALARS[name]
        cat = t.get("category") if t is not None else None
        if cat == "handle":
            if "VK_DEFINE_HANDLE" in text_of(t):
                return ("VkDispatch", "x", 8)
            return ("u64", "x", 8)
        if cat == "enum":
            return ("i32", "x", 4)
        if cat == "bitmask":
            base = t.find("type").text
            return SCALARS[base]
        if cat == "basetype" and t.find("type") is not None:
            return self.kind(t.find("type").text)
        raise SystemExit(f"unknown type {name}")


class Param:
    def __init__(self, reg, elem):
        self.name = elem.find("name").text
        self.type = elem.find("type").text
        text = text_of(elem)
        self.pointer = text.count("*") > 0 or "[" in text
        self.const = text.strip().startswith("const")
        self.allocator = self.type == "VkAllocationCallbacks"
        if self.pointer:
            self.rust = "*const c_void" if self.const else "*mut c_void"
            if self.type == "char":
                self.rust = "*const c_char" if self.const else "*mut c_char"
            self.cls, self.size = "x", 8
        else:
            self.rust, self.cls, self.size = reg.kind(self.type)


class Command:
    def __init__(self, reg, elem):
        proto = elem.find("proto")
        self.name = proto.find("name").text
        ret = proto.find("type").text
        self.fnptr = ret.startswith("PFN_")
        self.ret = None if ret == "void" or self.fnptr else (
            "i32" if ret == "VkResult" else reg.kind(ret)[0])
        self.params = [Param(reg, p) for p in elem.findall("param") if for_vulkan(p)]

    def layout(self):
        """Darwin arm64 argument placement: (nx, nd, stack words, per-param
        (kind, index or byte offset))."""
        nx = nd = stack = 0
        places = []
        for p in self.params:
            if p.cls == "x" and nx < 8:
                places.append(("x", nx)); nx += 1
            elif p.cls == "d" and nd < 8:
                places.append(("d", nd)); nd += 1
            else:
                # Apple's arm64 ABI packs stack arguments at their natural
                # alignment rather than in 8-byte slots.
                stack = (stack + p.size - 1) // p.size * p.size
                places.append(("s", stack)); stack += p.size
        return nx, nd, (stack + 7) // 8, places


def exports(dylib):
    out = subprocess.run(["nm", "-gU", dylib], check=True, capture_output=True, text=True).stdout
    return {line.split()[-1][1:] for line in out.splitlines() if line.split()}


def rust_name(name):
    return f"r#{name}" if name in {"type", "ref", "in", "fn", "match", "loop", "mod", "impl"} else name


def pack(c):
    """Rust statements filling `regs`, the host's register image."""
    nx, nd, ns, places = c.layout()
    total = nx + nd + ns
    if total == 0:
        return "let mut regs = [0u64; 0];", 0
    lines = [f"let mut regs = [0u64; {total}];"]
    for p, (kind, at) in zip(c.params, places):
        name = rust_name(p.name)
        if p.allocator:
            # Host code never calls guest code: the host allocates.
            continue
        if p.cls == "d":
            bits = f"{name}.to_bits() as u64"
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
            mask = "" if p.size == 8 else f" & {(1 << (p.size * 8)) - 1:#x}"
            shifted = f"({bits}{mask}) << {shift}" if shift else f"{bits}{mask}"
            lines.append(f"regs[{word}] |= {shifted};")
    return " ".join(lines), total


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--moltenvk", default=os.path.join(ROOT, "target/aim/moltenvk"))
    args = ap.parse_args()
    reg = Registry(os.path.join(args.moltenvk, "vk.xml"))
    have = exports(os.path.join(args.moltenvk, "libMoltenVK.dylib"))
    lock = dict(line.split("=", 1) for line in open(os.path.join(ROOT, "upstream/moltenvk.lock"))
                if "=" in line and not line.startswith("#"))
    source = (f"the Khronos registry vk.xml (Vulkan-Headers "
              f"{lock['VULKAN_HEADERS_REVISION'].strip()}) and MoltenVK "
              f"{lock['MOLTENVK_VERSION'].strip()}")

    names = {}  # canonical command -> names it is known by
    for alias, target in reg.aliases.items():
        names.setdefault(target, []).append(alias)
    table = []
    for name, elem in reg.commands.items():
        known = [name] + sorted(names.get(name, []))
        exported = [n for n in known if n in have]
        offered = [n for n in known if n in reg.offered]
        if not exported or not offered:
            continue
        if elem.find("proto/type").text.startswith("PFN_"):
            continue
        c = Command(reg, elem)
        # The host resolves the entry point by its first exported name.
        c.symbol = exported[0]
        c.names = offered
        table.append(c)
    table.sort(key=lambda c: c.name)

    digest = hashlib.sha256()
    for c in table:
        nx, nd, ns, _ = c.layout()
        digest.update(f"{c.name}:{c.symbol}:{nx}:{nd}:{ns};".encode())
    table_hash = int.from_bytes(digest.digest()[:8], "little")
    license = [
        f"// @generated by tools/gen-vulkan-thunks.py from {source}. Do not edit.",
        "//",
        "// The command signatures come from vk.xml:",
        f"// {reg.copyright}",
        "// SPDX-License-Identifier: Apache-2.0 OR MIT",
    ]

    host = license + [
        "",
        "use crate::Entry;",
        "",
        "/// Identifies this table; the guest driver must present the same.",
        f"pub const TABLE_HASH: u64 = {table_hash:#018x};",
        "",
        "/// Forwarded entry points, indexed by host-call function id",
        "/// minus `vulkan::FN_TABLE_BASE`.",
        "pub static ENTRIES: [Entry; %d] = [" % len(table),
    ]
    for c in table:
        nx, nd, ns, _ = c.layout()
        # Command recording creates no Objective-C objects; everything
        # else runs in an autorelease pool.
        pool = "false" if c.name.startswith("vkCmd") else "true"
        host.append(f'    Entry {{ name: c"{c.symbol}", nx: {nx}, nd: {nd}, ns: {ns}, pool: {pool} }},')
    host.append("];")

    guest = license + [
        "//",
        "// `host::*` makes the host call. `PROCS` is what vkGetInstanceProcAddr",
        "// and vkGetDeviceProcAddr hand out: these thunks, or the entry points",
        "// the driver implements itself.",
        "#![allow(non_snake_case, clippy::missing_safety_doc, clippy::too_many_arguments)]",
        "#![allow(clippy::unnecessary_cast, clippy::identity_op)]",
        "",
        "use core::ffi::{c_char, c_void};",
        "",
        "use crate::types::{Proc, VkDispatch};",
        "",
        f"pub const TABLE_HASH: u64 = {table_hash:#018x};",
        f"pub const TABLE_LEN: usize = {len(table)};",
        "",
        "/// Extensions of the host the driver does not offer.",
        "pub const WITHHELD_INSTANCE: &[&str] = &[%s];" % ", ".join(f'"{e}"' for e in WITHHELD_INSTANCE),
        "pub const WITHHELD_DEVICE: &[&str] = &[%s];" % ", ".join(f'"{e}"' for e in WITHHELD_DEVICE),
        "",
        "pub mod host {",
        "    use super::*;",
        "    use crate::hostcall::call;",
    ]
    for i, c in enumerate(table):
        params = ", ".join(f"{rust_name(p.name)}: {p.rust}" for p in c.params)
        body, n = pack(c)
        call = f"call({i}, regs.as_mut_ptr(), {n})"
        tail = f"{call} as {c.ret}" if c.ret else f"{call};"
        ret = f" -> {c.ret}" if c.ret else ""
        unused = any(p.allocator for p in c.params)
        guest.append("    #[inline]")
        if unused:
            guest.append("    #[allow(unused_variables)]")
        guest.append(f"    pub unsafe fn {c.name}({params}){ret} {{")
        guest.append(f"        {body}")
        guest.append(f"        unsafe {{ {tail} }}")
        guest.append("    }")
    guest.append("}")
    guest.append("")
    procs = {}
    for i, c in enumerate(table):
        params = ", ".join(f"{rust_name(p.name)}: {p.rust}" for p in c.params)
        args_ = ", ".join(rust_name(p.name) for p in c.params)
        ret = f" -> {c.ret}" if c.ret else ""
        if c.name not in GUEST:
            guest.append(f"unsafe extern \"C\" fn {c.name}({params}){ret} {{")
            guest.append(f"    unsafe {{ host::{c.name}({args_}) }}")
            guest.append("}")
        for n in c.names:
            procs[n] = (f"crate::{GUEST[c.name]}::{c.name}", -1) if c.name in GUEST else (c.name, i)
    for name, module in GUEST.items():
        procs.setdefault(name, (f"crate::{module}::{name}", -1))
    guest.append("")
    guest.append("/// Every entry point, sorted by name, with its host id (-1 for the")
    guest.append("/// driver's own).")
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
    print(f"{len(table)} entry points, {len(procs)} names, table hash {table_hash:#018x}")


if __name__ == "__main__":
    sys.exit(main())

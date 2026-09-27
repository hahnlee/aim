#!/usr/bin/env python3
"""Generates the sources and the ninja file that build ART for android-arm64.

The output is ordinary Android arm64 ELF (bionic ABI), compiled with the NDK
clang and linked against the ORIGINAL platform libraries of the pinned image.
Source lists come from ART's own Android.bp files (see bp_query.py); only the
configuration that Soong would compute from art/build/art.go is written here.

Called by tools/build-art-android.sh; not meant to be run by hand.
"""

import argparse
import glob
import json
import os
import re
import shlex
import subprocess
import sys

sys.dont_write_bytecode = True  # keep the source tree clean
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bp_query  # noqa: E402

# The fixed compressed-reference window. It must match the value the syscall
# layer keeps free and the value the boot image is generated for.
HEAP_REFERENCE_BASE = "0x0000010000000000"

# Soong's art.go global and device flags for an arm64 device build, with the
# concurrent-copying collector (Baker read barriers) compiled in, as the
# syscall layer does not provide userfaultfd.
ART_GLOBAL_CFLAGS = [
    "-O3",
    "-DART_DEFAULT_GC_TYPE_IS_CMC",
    "-DART_USE_READ_BARRIER=1",
    "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
    "-DART_USE_GENERATIONAL_GC=1",
    "-DART_FORCE_USE_READ_BARRIER=1",
    "-DART_USE_TLAB=1",
    "-DART_STACK_OVERFLOW_GAP_arm=8192",
    "-DART_STACK_OVERFLOW_GAP_arm64=8192",
    "-DART_STACK_OVERFLOW_GAP_riscv64=8192",
    "-DART_STACK_OVERFLOW_GAP_x86=8192",
    "-DART_STACK_OVERFLOW_GAP_x86_64=8192",
    "-DART_PAGE_SIZE_AGNOSTIC=1",
    "-DUSE_D8_DESUGAR=1",
    "-DART_FRAME_SIZE_LIMIT=1744",
    "-DART_BASE_ADDRESS=0x70000000",
    "-DART_BASE_ADDRESS_MIN_DELTA=(-0x1000000)",
    "-DART_BASE_ADDRESS_MAX_DELTA=0x1000000",
    "-DART_TARGET",
    "-DART_TARGET_ANDROID",
    "-DART_HEAP_REFERENCE_BASE=" + HEAP_REFERENCE_BASE,
]
ART_GLOBAL_ASFLAGS = [
    "-DART_USE_READ_BARRIER=1",
    "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
    "-DART_PAGE_SIZE_AGNOSTIC=1",
    "-DART_HEAP_REFERENCE_BASE=" + HEAP_REFERENCE_BASE,
]

# Soong's own device defaults that matter for correctness.
DEVICE_CFLAGS = [
    "-fPIC",
    "-ffunction-sections",
    "-fdata-sections",
    "-fno-exceptions",
    "-fstack-protector-strong",
    "-funwind-tables",
    "-fno-short-enums",
    "-fno-strict-aliasing",
    "-fno-omit-frame-pointer",
    # Soong zero-initializes automatic variables (and alloca) on every device
    # build; ART relies on it, e.g. LinkMethodsHelper's BitVector over an
    # alloca buffer in AssignVTableIndexes.
    "-ftrivial-auto-var-init=zero",
    "-D__ANDROID_APEX__",
    "-DANDROID",
    "-DNDEBUG",
    "-Wno-error",
    # The platform is built with a newer clang than the NDK's; warnings one of
    # them lacks must not stop the build.
    "-Wno-unknown-warning-option",
    "-Wno-unused-command-line-argument",
    "-Wno-deprecated-declarations",
    "-Wno-unused-parameter",
    "-Wno-missing-field-initializers",
]

BP_FILES = [
    "art/build/Android.bp",
    "art/runtime/Android.bp",
    "art/compiler/Android.bp",
    "art/libartbase/Android.bp",
    "art/libdexfile/Android.bp",
    "art/libprofile/Android.bp",
    "art/libelffile/Android.bp",
    "art/dex2oat/Android.bp",
]


def run(cmd, **kwargs):
    subprocess.run(cmd, check=True, **kwargs)


def write_if_changed(path, text):
    if os.path.exists(path):
        with open(path) as handle:
            if handle.read() == text:
                return
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as handle:
        handle.write(text)


class Build:
    def __init__(self, args):
        self.args = args
        self.out = os.path.abspath(args.out)
        self.gen = os.path.join(self.out, "gen")
        self.art = os.path.abspath(args.art)
        self.src_root = os.path.dirname(self.art)  # contains art/ and fetched trees
        self.aosp = os.path.abspath(args.aosp)
        self.image = os.path.abspath(args.image)
        self.ndk = os.path.abspath(args.ndk)
        self.toolchain = os.path.join(self.ndk, "toolchains/llvm/prebuilt/darwin-x86_64")
        self.lines = []
        self.modules = bp_query.load(self.src_root, BP_FILES)

    # -- helpers ------------------------------------------------------------

    def aosp_path(self, *parts):
        return os.path.join(self.aosp, *parts)

    def fetched(self, *parts):
        return os.path.join(self.src_root, *parts)

    def query(self, module, prop):
        values = bp_query.resolve(self.modules, module).get(prop, [])
        seen, result = set(), []
        for value in values:
            if value not in seen:
                seen.add(value)
                result.append(value)
        return result

    def srcs(self, module, root=None):
        root = root or self.src_root
        excluded = set(self.query(module, "exclude_srcs"))
        result = []
        for src in self.query(module, "srcs"):
            if src.startswith(":") or src in excluded:
                continue
            if "*" in src:
                result.extend(sorted(glob.glob(os.path.join(root, src))))
            else:
                result.append(os.path.join(root, src))
        return result

    # -- generated sources --------------------------------------------------

    def operator_srcs(self, gensrcs_name, prefix):
        kind, body, directory = self.modules[gensrcs_name]
        outputs = []
        script = os.path.join(self.art, "tools/generate_operator_out.py")
        for header in body["srcs"]:
            source = os.path.join(directory, header)  # relative: sets the #include
            output = os.path.join(self.gen, "operator_out", directory,
                                  header.replace(".h", ".operator_out.cc"))
            text = subprocess.run([sys.executable, script, prefix, source], check=True,
                                  capture_output=True, text=True, cwd=self.src_root).stdout
            write_if_changed(output, text)
            outputs.append(output)
        return outputs

    def mterp(self):
        output = os.path.join(self.gen, "mterp/mterp_arm64ng.S")
        inputs = sorted(glob.glob(os.path.join(self.art, "runtime/interpreter/mterp/arm64ng/*.S")))
        os.makedirs(os.path.dirname(output), exist_ok=True)
        run([sys.executable, os.path.join(self.art, "runtime/interpreter/mterp/gen_mterp.py"),
             output] + inputs)
        return output

    def aconfig(self):
        """Emits the fixed-read-only flag header that cc_aconfig_library makes.

        Every ART flag is `is_fixed_read_only`, so aconfig's generated C++ is a
        set of constant functions. Values are the declarations' defaults
        (disabled): this build has no release configuration.
        """
        declarations = open(os.path.join(self.art, "build/flags/art-flags.aconfig")).read()
        names = re.findall(r'^\s*name:\s*"([a-z0-9_]+)"', declarations, re.M)
        lines = ["#pragma once", "// Generated from art/build/flags/art-flags.aconfig.",
                 "#ifdef __cplusplus", "namespace com::android::art::flags {"]
        for name in names:
            lines.append(f"constexpr inline bool {name}() {{ return false; }}")
        lines += ["}  // namespace com::android::art::flags", "#endif"]
        for name in names:
            lines.append(f"#define COM_ANDROID_ART_FLAGS_{name.upper()} false")
        write_if_changed(os.path.join(self.gen, "include/com_android_art_flags.h"),
                         "\n".join(lines) + "\n")

    def xsdc(self, schema, package, root, name):
        out = os.path.join(self.gen, "xsdc", name)
        stamp = os.path.join(out, ".stamp")
        if not os.path.exists(stamp):
            os.makedirs(out, exist_ok=True)
            classpath = ":".join([os.path.join(self.args.xsdc, "xsdc.jar"),
                                  os.path.join(self.args.xsdc, "commons-cli-1.2.jar")])
            cmd = [self.args.java, "-cp", classpath, "com.android.xsdc.Main", "-c", "-w", "-t",
                   "-p", package, "-o", out, schema]
            if root:
                cmd[cmd.index("-o"):cmd.index("-o")] = ["-r", root]
            run(cmd)
            open(stamp, "w").close()
        return (os.path.join(out, "include"),
                sorted(glob.glob(os.path.join(out, "*.cpp"))))

    # -- flags --------------------------------------------------------------

    def include_dirs(self):
        art = self.art
        a = self.aosp_path
        f = self.fetched
        return [
            os.path.join(self.gen, "include"),
            os.path.join(self.gen, "asm_defines"),
            os.path.join(self.gen, "xsdc/apex/include"),
            os.path.join(self.gen, "xsdc/art/include"),
            os.path.join(art, "runtime"),
            os.path.join(art, "compiler"),
            os.path.join(art, "compiler/export"),
            os.path.join(art, "dex2oat"),
            os.path.join(art, "dex2oat/include"),
            os.path.join(art, "disassembler"),
            os.path.join(art, "libartbase"),
            os.path.join(art, "libdexfile"),
            os.path.join(art, "libdexfile/external/include"),
            os.path.join(art, "libprofile"),
            os.path.join(art, "libelffile"),
            os.path.join(art, "cmdline"),
            os.path.join(art, "libartpalette/include"),
            os.path.join(art, "libnativebridge/include"),
            os.path.join(art, "libnativeloader/include"),
            os.path.join(art, "sigchainlib"),
            os.path.join(art, "odrefresh/include"),
            os.path.join(art, "tools/cpp-define-generator"),
            a("external/vixl/src"),
            a("external/lzma/C"),
            a("external/zlib"),
            a("external/fmtlib/include"),
            a("external/tinyxml2"),
            a("external/dlmalloc"),
            a("system/libbase/include"),
            a("system/logging/liblog/include"),
            a("system/libziparchive/include"),
            a("system/libziparchive/incfs_support/include"),
            a("system/core/libcutils/include"),
            a("system/unwinding/libunwindstack/include"),
            a("libnativehelper/include"),
            a("libnativehelper/include_jni"),
            a("libnativehelper/header_only_include"),
            a("libnativehelper/platform_header_only_include"),
            a("external/googletest/googletest/include"),  # gtest_prod.h (FRIEND_TEST)
            f("external/lz4/lib"),
            f("external/cpu_features/include"),
            f("external/libcap/libcap/include"),
            f("external/perfetto/src/profiling/memory/include"),
            f("frameworks/libs/modules-utils/build/include"),
            f("packages/modules/StatsD/lib/libstatssocket/include"),
            f("packages/modules/StatsD/lib/libstatspull/include"),
            f("bionic/libc/platform"),
            f("bionic/libc/async_safe/include"),
            a("boringssl-full/src/include"),  # <openssl/sha.h> for dex2oat's build id
        ]

    def libcxx_config(self):
        """Selects libc++'s `__1` ABI namespace, which the platform libc++.so exports.

        The NDK headers default to `__ndk1`; the image's libraries (libbase,
        libunwindstack, ...) take `std::__1` types across their C++ APIs.
        """
        ndk_site = os.path.join(self.toolchain, "sysroot/usr/include/c++/v1/__config_site")
        text = open(ndk_site).read().replace("#define _LIBCPP_ABI_NAMESPACE __ndk1",
                                             "#define _LIBCPP_ABI_NAMESPACE __1")
        write_if_changed(os.path.join(self.gen, "libcxx/__config_site"), text)
        return os.path.join(self.gen, "libcxx")

    # -- ninja --------------------------------------------------------------

    def emit(self, line=""):
        self.lines.append(line)

    def rules(self):
        clang = os.path.join(self.toolchain, "bin/clang")
        target = f"--target=aarch64-linux-android{self.args.api}"
        self.emit(f"cc = {clang}")
        self.emit(f"cxx = {clang}++")
        self.emit(f"ar = {os.path.join(self.toolchain, 'bin/llvm-ar')}")
        self.emit(f"target = {target}")
        self.emit("rule cxx\n  command = $cxx $target -MMD -MF $out.d $flags -std=gnu++20 -c $in -o $out\n"
                  "  depfile = $out.d\n  deps = gcc\n  description = CXX $out")
        self.emit("rule cc\n  command = $cc $target -MMD -MF $out.d $flags -std=gnu17 -c $in -o $out\n"
                  "  depfile = $out.d\n  deps = gcc\n  description = CC $out")
        self.emit("rule asm\n  command = $cc $target -MMD -MF $out.d $flags -c $in -o $out\n"
                  "  depfile = $out.d\n  deps = gcc\n  description = AS $out")
        self.emit("rule ar\n  command = rm -f $out && $ar crsD $out $in\n  description = AR $out")
        self.emit("rule link\n  command = $cxx $target -shared -nostdlib++ -Wl,-soname,$soname "
                  "-Wl,--build-id=sha1 -Wl,-z,defs -Wl,--no-undefined-version -Wl,--gc-sections "
                  "-o $out $in $libs $ldflags\n  description = LINK $out")
        self.emit("rule link_exe\n  command = $cxx $target -pie -nostdlib++ "
                  "-Wl,--build-id=sha1 -Wl,-z,defs -Wl,--gc-sections "
                  "-o $out $in $libs $ldflags\n  description = LINK $out")

    def objects(self, name, sources, flags):
        objs = []
        for source in sources:
            rel = os.path.relpath(source, "/")
            obj = os.path.join(self.out, "obj", name, rel) + ".o"
            if source.endswith((".cc", ".cpp")):
                rule = "cxx"
            elif source.endswith(".c"):
                rule = "cc"
            elif source.endswith(".S"):
                rule = "asm"
            else:
                raise ValueError(source)
            self.emit(f"build {obj}: {rule} {source}")
            self.emit(f"  flags = {flags[rule]}")
            objs.append(obj)
        return objs

    def static_lib(self, name, sources, flags):
        objs = self.objects(name, sources, flags)
        lib = os.path.join(self.out, "obj", f"{name}.a")
        self.emit(f"build {lib}: ar {' '.join(objs)}")
        return lib

    def shared_lib(self, name, objs, static_libs, shared_libs, extra_ldflags=""):
        out = os.path.join(self.out, "lib64", f"{name}.so")
        whole = " ".join(static_libs)
        libs = f"-Wl,--whole-archive {whole} -Wl,--no-whole-archive" if static_libs else ""
        self.emit(f"build {out}: link {' '.join(objs)} | {' '.join(static_libs + shared_libs)}")
        self.emit(f"  soname = {name}.so")
        self.emit(f"  libs = {libs} {' '.join(shared_libs)}")
        self.emit(f"  ldflags = {extra_ldflags}")
        return out

    def executable(self, name, objs, static_libs, shared_libs):
        out = os.path.join(self.out, "bin", name)
        self.emit(f"build {out}: link_exe {' '.join(objs)} | {' '.join(static_libs + shared_libs)}")
        self.emit(f"  libs = {' '.join(static_libs)} {' '.join(shared_libs)}")
        self.emit("  ldflags = ")
        return out

    def image_lib(self, *candidates):
        for candidate in candidates:
            path = os.path.join(self.image, candidate)
            if os.path.exists(path):
                return path
        raise FileNotFoundError(candidates)

    # -- main ---------------------------------------------------------------

    def generate(self):
        os.makedirs(self.gen, exist_ok=True)
        libcxx = self.libcxx_config()
        self.aconfig()
        apex_include, apex_srcs = self.xsdc(self.args.apex_xsd, "com.android.apex",
                                            "apex-info-list", "apex")
        art_include, art_srcs = self.xsdc(os.path.join(self.art, "odrefresh/CacheInfo.xsd"),
                                          "com.android.art", None, "art")

        includes = [libcxx] + self.include_dirs()
        include_flags = " ".join(f"-I{d}" for d in includes)

        def flags_for(module, extra=()):
            module_cflags = [f for f in self.query(module, "cflags") if f != "-Werror"]
            cflags = " ".join(shlex.quote(f) for f in
                              DEVICE_CFLAGS + ART_GLOBAL_CFLAGS + module_cflags + list(extra))
            asflags = " ".join(ART_GLOBAL_ASFLAGS + self.query(module, "asflags"))
            return {"cxx": f"{include_flags} {cflags}", "cc": f"{include_flags} {cflags}",
                    "asm": f"{include_flags} {asflags} -D__ASSEMBLY__"}

        external_flags = {
            "cxx": f"{include_flags} {' '.join(DEVICE_CFLAGS)} -O2",
            "cc": f"{include_flags} {' '.join(DEVICE_CFLAGS)} -O2",
            "asm": include_flags,
        }

        self.rules()
        self.asm_defines(includes, flags_for("libart"))

        # Image libraries (read-only): the ORIGINAL platform libraries.
        img = self.image_lib
        libc = [img("apex/com.android.runtime/lib64/bionic/libc.so"),
                img("apex/com.android.runtime/lib64/bionic/libm.so"),
                img("apex/com.android.runtime/lib64/bionic/libdl.so")]
        libcxx_so = img("apex/com.android.art/lib64/libc++.so")
        libbase = img("apex/com.android.art/lib64/libbase.so")
        liblog = img("system/lib64/liblog.so")
        libz = img("system/lib64/libz.so")
        palette = img("apex/com.android.art/lib64/libartpalette.so")
        art_apex = lambda n: img(f"apex/com.android.art/lib64/{n}")

        # Static third-party pieces that ART links whole.
        ziparchive = self.static_lib(
            "libziparchive",
            [self.aosp_path("system/libziparchive", s) for s in
             ("zip_archive.cc", "zip_archive_stream_entry.cc", "zip_cd_entry_map.cc",
              "zip_error.cpp", "zip_writer.cc")],
            {**external_flags,
             "cxx": external_flags["cxx"] + " -DZLIB_CONST -D_FILE_OFFSET_BITS=64"
                                            " -DINCFS_SUPPORT_DISABLED=1"})
        tinyxml = self.static_lib("libtinyxml2", [self.aosp_path("external/tinyxml2/tinyxml2.cpp")],
                                  external_flags)
        libcap = self.static_lib(
            "libcap",
            [self.fetched("external/libcap/libcap", s) for s in
             ("cap_alloc.c", "cap_extint.c", "cap_file.c", "cap_flag.c", "cap_proc.c",
              "cap_text.c")],
            {**external_flags,
             "cc": external_flags["cc"] + f" -I{self.gen}/libcap -Wno-pointer-arith"
                                          " -Wno-tautological-compare -Wno-unused-result"})
        self.libcap_names()
        cpu_features = self.static_lib(
            "libcpu_features",
            [self.fetched("external/cpu_features/src", s) for s in
             ("impl_aarch64_linux_or_android.c", "filesystem.c", "hwcaps.c",
              "stack_line_reader.c", "string_view.c")],
            {**external_flags,
             "cc": external_flags["cc"] + " -DSTACK_LINE_READER_BUFFER_SIZE=1024 -DHAVE_DLFCN_H"
                                          " -DHAVE_STRONG_GETAUXVAL -Wno-gnu-designator"
                                          f" -I{self.fetched('external/cpu_features/include/internal')}"})
        vixl = self.static_lib(
            "libvixl",
            # external/vixl/Android.bp: vixl-common + vixl-arm + vixl-arm64.
            sorted(glob.glob(self.aosp_path("external/vixl/src/*.cc")) +
                   glob.glob(self.aosp_path("external/vixl/src/aarch32/*.cc")) +
                   glob.glob(self.aosp_path("external/vixl/src/aarch64/*.cc"))),
            {**external_flags,
             "cxx": external_flags["cxx"] +
                    " -DVIXL_GENERATE_SIMULATOR_INSTRUCTIONS_VALUE=0 -DVIXL_CODE_BUFFER_MALLOC"
                    " -DVIXL_INCLUDE_TARGET_T32 -DVIXL_INCLUDE_SIMULATOR_AARCH64"
                    " -DVIXL_INCLUDE_TARGET_A64 -Wno-missing-noreturn"})

        # libartbase
        artbase_flags = flags_for("libartbase", ["-DBUILDING_LIBARTBASE"])
        artbase_objs = self.objects(
            "libartbase",
            self.srcs("libartbase") +
            self.operator_srcs("art_libartbase_operator_srcs", "art/libartbase"),
            artbase_flags)
        artbase = self.shared_lib("libartbase", artbase_objs, [ziparchive, tinyxml, libcap],
                                  [libz, liblog, palette, libbase, libcxx_so] + libc)

        # libdexfile
        dexfile_objs = self.objects(
            "libdexfile",
            self.srcs("libdexfile") +
            self.operator_srcs("dexfile_operator_srcs", "art/libdexfile"),
            flags_for("libdexfile"))
        dexfile = self.shared_lib("libdexfile", dexfile_objs, [ziparchive],
                                  [artbase, libz, liblog, palette, libbase, libcxx_so] + libc)

        # libprofile
        profile_objs = self.objects("libprofile", self.srcs("libprofile"), flags_for("libprofile"))
        profile = self.shared_lib("libprofile", profile_objs, [ziparchive],
                                  [artbase, dexfile, libz, liblog, palette, libbase,
                                   libcxx_so] + libc)

        # libelffile (static, into libart)
        elffile = self.static_lib("libelffile", self.srcs("libelffile"), flags_for("libelffile"))

        # libart = libart-compiler + libart-runtime
        runtime_flags = flags_for("libart-runtime")
        runtime_srcs = self.srcs("libart-runtime") + [self.mterp()] + apex_srcs + art_srcs
        runtime_srcs += self.operator_srcs("art_operator_srcs", "art/runtime")
        # Upstream's host variant of odrefresh metrics (no statsd pulled atoms).
        runtime_srcs.append(os.path.join(self.art, "odrefresh/odr_statslog_host.cc"))
        runtime_srcs = [s for s in runtime_srcs if not s.endswith("metrics/statsd.cc")]
        runtime_srcs.append(os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                         "statsd_unavailable.cc"))
        runtime_objs = self.objects("libart-runtime", runtime_srcs, runtime_flags)

        compiler_flags = flags_for("libart-compiler")
        compiler_srcs = self.srcs("libart-compiler") + \
            self.operator_srcs("art_compiler_operator_srcs", "art/compiler")
        compiler_objs = self.objects("libart-compiler", compiler_srcs, compiler_flags)

        art_shared = [artbase, dexfile, profile, palette, libbase, liblog,
                      art_apex("liblz4.so"), art_apex("liblzma.so"),
                      art_apex("libnativebridge.so"), art_apex("libnativeloader.so"),
                      art_apex("libsigchain.so"), art_apex("libunwindstack.so"),
                      img("apex/com.android.runtime/lib64/bionic/libdl_android.so"),
                      img("apex/com.android.os.statsd/lib64/libstatspull.so"),
                      img("apex/com.android.os.statsd/lib64/libstatssocket.so"),
                      libz, img("system/lib64/heapprofd_client_api.so"), libcxx_so] + libc
        libart = self.shared_lib("libart", runtime_objs + compiler_objs,
                        [elffile, vixl, cpu_features], art_shared,
                        "-Wl,--keep-unique,__jit_debug_register_code"
                        " -Wl,--keep-unique,__dex_debug_register_code")

        # libopenjdkjvm: the JVM_* interface libopenjdk.so calls; it uses libart
        # internals, so it must be rebuilt with the patched runtime.
        jvm_objs = self.objects(
            "libopenjdkjvm",
            [self.aosp_path("art-openjdkjvm/art/openjdkjvm/OpenjdkJvm.cc")],
            flags_for("libelffile"))  # art_defaults only: not BUILDING_LIBART
        self.shared_lib("libopenjdkjvm", jvm_objs, [],
                        [libart, artbase, libbase, liblog, libcxx_so] + libc)

        # dex2oat64: regenerates the boot image with the same reference encoding.
        # It links BoringSSL's libcrypto_static (for the SHA-1 build id) as
        # upstream does: the ART namespace cannot see the platform libcrypto.so.
        crypto = self.libcrypto_static(includes)
        dex2oat_flags = flags_for("libart-dex2oat")
        dex2oat_objs = self.objects(
            "dex2oat",
            self.srcs("libart-dex2oat") +
            self.operator_srcs("art_dex2oat_operator_srcs", "art/dex2oat") +
            [os.path.join(self.art, "dex2oat", s) for s in ("dex2oat_options.cc", "dex2oat.cc")],
            dex2oat_flags)
        self.executable("dex2oat64", dex2oat_objs, [elffile, crypto],
                        [libart, artbase, dexfile, profile, palette, libbase,
                         art_apex("liblz4.so"), liblog, art_apex("libsigchain.so"), libz,
                         libcxx_so] + libc)

        write_if_changed(os.path.join(self.out, "build.ninja"), "\n".join(self.lines) + "\n")

    def libcrypto_static(self, includes):
        """BoringSSL's libcrypto_static (external/boringssl/Android.bp): the
        bcm and crypto sources and their Linux assembly (each file is guarded
        by its architecture), without FIPS self tests."""
        root = self.aosp_path("boringssl-full/src")
        with open(os.path.join(root, "gen/sources.json")) as handle:
            sources = json.load(handle)
        files = sources["bcm"]["srcs"] + sources["crypto"]["srcs"] + [
            s for s in sources["bcm"]["asm"] + sources["crypto"]["asm"]
            if not s.endswith(("-apple.S", "-win.S"))]
        defines = ("-DBORINGSSL_IMPLEMENTATION -DBORINGSSL_ANDROID_SYSTEM -DOPENSSL_SMALL "
                   "-fvisibility=hidden")
        include = f"-I{os.path.join(root, 'include')} -I{includes[0]}"
        base = f"{include} {' '.join(DEVICE_CFLAGS)} -O2 {defines}"
        flags = {"cxx": base, "cc": base, "asm": f"{include} {defines}"}
        return self.static_lib("libcrypto_static",
                               [os.path.join(root, s) for s in files], flags)

    def libcap_names(self):
        """libcap's cap_names.h, made the way external/libcap/Android.bp does.

        awk turns <linux/capability.h> into cap_names.list.h, then the host
        tool `_makenames` prints cap_names.h.
        """
        libcap = self.fetched("external/libcap")
        out = os.path.join(self.gen, "libcap")
        os.makedirs(out, exist_ok=True)
        with open(os.path.join(out, "cap_names.list.h"), "w") as handle:
            run(["awk", "-f", os.path.join(libcap, "generate_cap_names_list.awk"),
                 os.path.join(libcap, "libcap/include/uapi/linux/capability.h")], stdout=handle)
        tool = os.path.join(out, "_makenames")
        run(["xcrun", "clang", "-I", out, "-I", os.path.join(libcap, "libcap/include"),
             os.path.join(libcap, "libcap/_makenames.c"), "-o", tool])
        text = subprocess.run([tool], check=True, capture_output=True, text=True).stdout
        write_if_changed(os.path.join(out, "cap_names.h"), text)

    def asm_defines(self, includes, flags):
        """cpp-define-generator: compile asm_defines.cc to assembly, then extract the constants."""
        out_dir = os.path.join(self.gen, "asm_defines")
        header = os.path.join(out_dir, "asm_defines.h")
        os.makedirs(out_dir, exist_ok=True)
        assembly = os.path.join(out_dir, "asm_defines.s")
        clang = os.path.join(self.toolchain, "bin/clang++")
        cmd = ([clang, f"--target=aarch64-linux-android{self.args.api}", "-std=gnu++20", "-S",
                "-Wno-error"] + shlex.split(flags["cxx"]) +
               [os.path.join(self.art, "tools/cpp-define-generator/asm_defines.cc"),
                "-o", assembly])
        run(cmd)
        text = subprocess.run([sys.executable,
                               os.path.join(self.art, "tools/cpp-define-generator/make_header.py"),
                               assembly], check=True, capture_output=True, text=True).stdout
        write_if_changed(header, text)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--art", required=True, help="patched ART tree (<src>/art)")
    parser.add_argument("--aosp", required=True, help="pinned AOSP sources (_aosp)")
    parser.add_argument("--image", required=True, help="extracted original image")
    parser.add_argument("--ndk", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--xsdc", required=True, help="directory with xsdc.jar")
    parser.add_argument("--apex-xsd", required=True)
    parser.add_argument("--java", default="java")
    parser.add_argument("--api", default="35")
    Build(parser.parse_args()).generate()


if __name__ == "__main__":
    main()

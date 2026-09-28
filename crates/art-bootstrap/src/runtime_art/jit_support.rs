use super::*;
use crate::native_build::{PendingNativeCompile, common_cpp_command, compile_pending_native};

const LIBELFFILE_CXX_SOURCES: &[&str] = &[
    "elf/xz_utils.cc",
    "stream/buffered_output_stream.cc",
    "stream/file_output_stream.cc",
    "stream/output_stream.cc",
    "stream/vector_output_stream.cc",
];

// Keep this list aligned with external/lzma's Android.bp.  libelffile uses
// the XZ encoder/decoder directly, so linking only a system liblzma would
// silently change the pinned ART ABI and compression behavior.
const LZMA_C_SOURCES: &[&str] = &[
    "C/7zAlloc.c",
    "C/7zArcIn.c",
    "C/7zBuf2.c",
    "C/7zBuf.c",
    "C/7zCrc.c",
    "C/7zCrcOpt.c",
    "C/7zDec.c",
    "C/7zFile.c",
    "C/7zStream.c",
    "C/Aes.c",
    "C/AesOpt.c",
    "C/Alloc.c",
    "C/Bcj2.c",
    "C/Bra86.c",
    "C/Bra.c",
    "C/BraIA64.c",
    "C/CpuArch.c",
    "C/Delta.c",
    "C/LzFind.c",
    "C/Lzma2Dec.c",
    "C/Lzma2Enc.c",
    "C/Lzma86Dec.c",
    "C/Lzma86Enc.c",
    "C/LzmaDec.c",
    "C/LzmaEnc.c",
    "C/LzmaLib.c",
    "C/Ppmd7.c",
    "C/Ppmd7Dec.c",
    "C/Ppmd7Enc.c",
    "C/Sha256.c",
    "C/Sha256Opt.c",
    "C/Sort.c",
    "C/Xz.c",
    "C/XzCrc64.c",
    "C/XzCrc64Opt.c",
    "C/XzDec.c",
    "C/XzEnc.c",
    "C/XzIn.c",
];

/// Build the exact AOSP libelffile closure needed by compiler debug ELF
/// emission. The result is self-contained: the five C++ libelffile sources
/// and the pinned 7-Zip/LZMA C implementation are archived together, so the
/// final ART link does not depend on a host package or a stub implementation.
pub(crate) fn build_jit_libelffile(root: &Path) -> Result<PathBuf> {
    let libelffile = root.join("_aosp/art/libelffile");
    let lzma = root.join("_aosp/external/lzma");
    for required in [
        libelffile.join("Android.bp"),
        lzma.join("Android.bp"),
        libelffile.join("elf/xz_utils.cc"),
    ] {
        if !required.is_file() {
            return Err(format!(
                "JIT libelffile closure source is missing: {}; run `art-bootstrap sync-jit-sources` first",
                required.display()
            )
            .into());
        }
    }

    let build = root.join("_build/jit-compiler/libelffile");
    let object_dir = build.join("objects");
    fs::create_dir_all(&object_dir)?;

    let include_paths = [
        root.join("include"),
        root.join("compat"),
        root.join("_build/foundation/patched-source/libartbase"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/libelffile"),
        root.join("_aosp/art/runtime"),
        root.join("_aosp/art"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/libnativehelper/include_jni"),
        root.join("_aosp/libnativehelper/header_only_include"),
        root.join("_aosp/external/fmtlib/include"),
        lzma.join("C"),
    ];
    let includes: Vec<&Path> = include_paths.iter().map(PathBuf::as_path).collect();
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let mut jobs = Vec::with_capacity(LIBELFFILE_CXX_SOURCES.len() + LZMA_C_SOURCES.len());

    for source in LIBELFFILE_CXX_SOURCES {
        let source_path = libelffile.join(source);
        let object = object_dir.join(format!("libelffile_{}.o", source.replace('/', "_")));
        let mut command = common_cpp_command(&includes);
        command
            .arg("-std=gnu++20")
            .arg("-O2")
            .arg("-DNDEBUG")
            .arg("-c")
            .arg(source_path)
            .arg("-o")
            .arg(&object);
        jobs.push(PendingNativeCompile { command, object });
    }

    for source in LZMA_C_SOURCES {
        let source_path = lzma.join(source);
        let object = object_dir.join(format!("lzma_{}.o", source.replace('/', "_")));
        let mut command = Command::new("clang");
        command.args([
            "-std=gnu11",
            "-O2",
            "-DNDEBUG",
            "-DZ7_ST",
            "-Wall",
            "-Werror",
            "-Wno-empty-body",
            "-Wno-enum-conversion",
            "-Wno-logical-op-parentheses",
            "-Wno-self-assign",
            "-ffunction-sections",
            "-fdata-sections",
        ]);
        command
            .arg("-I")
            .arg(lzma.join("C"))
            .arg("-c")
            .arg(source_path)
            .arg("-o")
            .arg(&object);
        jobs.push(PendingNativeCompile { command, object });
    }

    let (objects, compiled, cached) = compile_pending_native(jobs, &compiler_identity)?;
    let archive = root.join("_build/jit-compiler/libart-libelffile-darwin.a");
    create_archive(&archive, &objects)?;
    println!(
        "build-jit-libelffile: AOSP libelffile+LZMA objects={} compiled={compiled} cached={cached} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(archive)
}

use std::path::Path;
use std::process::Command;

use crate::native_build::common_cpp_command;

pub(crate) fn runtime_cpp_command(includes: &[&Path]) -> Command {
    let mut command = common_cpp_command(includes);
    command.args([
        "-DBUILDING_LIBART",
        "-DUSE_D8_DESUGAR",
        "-DART_DEFAULT_GC_TYPE_IS_CMS",
        // Darwin has no Linux userfaultfd collector. Use AOSP's normal Baker
        // read-barrier/ConcurrentCopying path for the production runtime
        // instead of compiling a separate no-read-barrier object model.
        "-DART_USE_READ_BARRIER",
        "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
        "-DART_FORCE_USE_READ_BARRIER",
        "-DART_FRAME_SIZE_LIMIT=1744",
        "-DART_BASE_ADDRESS=0x70000000",
        // Assembly JNI/trampoline entrypoints use this explicit host window
        // constant to lift logical 32-bit ArtMethod/reference values before
        // native dereferences. Keep it aligned with kArtCompressedReferenceBase.
        "-DAIM_REFERENCE_BASE=0x10000000000",
        "-DART_BASE_ADDRESS_MIN_DELTA=(-0x1000000)",
        "-DART_BASE_ADDRESS_MAX_DELTA=0x1000000",
        "-DART_STACK_OVERFLOW_GAP_arm=8192",
        "-DART_STACK_OVERFLOW_GAP_arm64=8192",
        "-DART_STACK_OVERFLOW_GAP_riscv64=8192",
        "-DART_STACK_OVERFLOW_GAP_x86=8192",
        "-DART_STACK_OVERFLOW_GAP_x86_64=8192",
        "-Wno-invalid-offsetof",
        "-Wno-unsupported-visibility",
        "-Wno-deprecated-enum-enum-conversion",
        "-Wno-nontrivial-memcall",
    ]);
    command
}

pub(crate) fn runtime_bootstrap_cpp_command(includes: &[&Path]) -> Command {
    let mut command = runtime_cpp_command(includes);
    command.args([
        "-include",
        "mirror/object_reference.h",
        "-include",
        "mirror/string-inl.h",
    ]);
    command
}

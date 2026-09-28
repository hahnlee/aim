use super::*;
use crate::native_build::{PendingNativeCompile, common_cpp_command, compile_pending_native};

// Architecture-neutral AOSP libunwindstack translation units. Darwin-specific
// Maps, Memory, Regs, AndroidUnwinder and ThreadUnwinder providers are kept out
// of this archive so platform work cannot silently fork the DWARF/ELF engine.
const PORTABLE_SOURCES: &[&str] = &[
    "ArmExidx.cpp",
    "DwarfCfa.cpp",
    "DwarfEhFrameWithHdr.cpp",
    "DwarfMemory.cpp",
    "DwarfOp.cpp",
    "DwarfSection.cpp",
    "Elf.cpp",
    "ElfInterface.cpp",
    "ElfInterfaceArm.cpp",
    "JitDebug.cpp",
    "MapInfo.cpp",
    "MemoryXz.cpp",
    "RegsArm.cpp",
    "RegsX86.cpp",
    "RegsX86_64.cpp",
    "Symbols.cpp",
    "Unwinder.cpp",
    "LogStdout.cpp",
];

const DARWIN_PROVIDER_SOURCES: &[&str] = &[
    "AndroidUnwinder.cpp",
    "Demangle.cpp",
    "Global.cpp",
    "Maps.cpp",
    "Memory.cpp",
    "MemoryMte.cpp",
    "Regs.cpp",
    "RegsArm64.cpp",
    "RegsRiscv64.cpp",
    "ThreadEntry.cpp",
    "ThreadUnwinder.cpp",
    "darwin_unwindstack_native.cc",
];

pub(crate) fn build_runtime_unwindstack_core(root: &Path) -> Result<PathBuf> {
    let source = root.join("_aosp/system/unwinding/libunwindstack");
    if !source.join("Android.bp").is_file() {
        return Err(
            "full pinned AOSP libunwindstack source is missing; run `art-bootstrap sync`".into(),
        );
    }
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let build = root.join("_build/runtime-unwindstack");
    let rust_target = build.join("rust-demangle-target");
    run_command(
        Command::new("cargo")
            .args([
                "build",
                "-q",
                "-p",
                "aim-rust-demangle",
                "--release",
                "--target-dir",
            ])
            .arg(&rust_target)
            .current_dir(root),
    )?;
    let rust_demangle_archive = rust_target.join("release/libaim_rust_demangle.a");
    if !rust_demangle_archive.is_file() {
        return Err("Rust unwind demangler provider archive was not produced".into());
    }
    let object_dir = build.join("portable-objects");
    fs::create_dir_all(&object_dir)?;

    let include_paths = [
        root.join("compat"),
        source.join("include"),
        source.clone(),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/system/logging/liblog/include"),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/libdexfile"),
        root.join("_aosp/art/libdexfile/external/include"),
        root.join("_aosp/external/lzma/C"),
        root.join("_aosp/external/zlib"),
        PathBuf::from("/opt/homebrew/include"),
    ];
    let includes: Vec<&Path> = include_paths.iter().map(PathBuf::as_path).collect();
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let jobs = PORTABLE_SOURCES
        .iter()
        .map(|name| {
            let object = object_dir.join(format!("{}.o", name.replace('/', "_")));
            let mut command = common_cpp_command(&includes);
            command
                .arg("-std=gnu++20")
                .arg("-O2")
                .arg("-DNDEBUG")
                // Darwin provides its native libc headers first. The pinned
                // NDK contributes only Linux ELF ABI declarations unavailable
                // in the macOS SDK.
                .arg("-idirafter")
                .arg(&ndk_arch_include)
                .arg("-idirafter")
                .arg(&ndk_include)
                .arg("-c")
                .arg(source.join(name))
                .arg("-o")
                .arg(&object);
            PendingNativeCompile { command, object }
        })
        .collect::<Vec<_>>();
    let (objects, compiled, cached) = compile_pending_native(jobs, &compiler_identity)?;
    if objects.len() != PORTABLE_SOURCES.len() {
        return Err(format!(
            "AOSP unwindstack portable object inventory drift: {}/{}",
            objects.len(),
            PORTABLE_SOURCES.len()
        )
        .into());
    }
    let archive = build.join("libunwindstack-core-darwin.a");
    create_archive(&archive, &objects)?;

    let provider_object_dir = build.join("provider-objects");
    let provider_source_dir = build.join("provider-sources");
    fs::create_dir_all(&provider_object_dir)?;
    fs::create_dir_all(&provider_source_dir)?;
    let provider_jobs = DARWIN_PROVIDER_SOURCES
        .iter()
        .map(|name| {
            let upstream_source = if *name == "darwin_unwindstack_native.cc" {
                root.join("compat/darwin_unwindstack_native.cc")
            } else {
                source.join(name)
            };
            let source_path = if matches!(
                *name,
                "AndroidUnwinder.cpp"
                    | "Global.cpp"
                    | "Memory.cpp"
                    | "MemoryMte.cpp"
                    | "RegsArm64.cpp"
            ) {
                let generated = provider_source_dir.join(name);
                let contents = fs::read_to_string(&upstream_source)?;
                let patched = if *name == "AndroidUnwinder.cpp" {
                    let ucontext_signature =
                        "bool AndroidUnwinder::Unwind(void* ucontext, AndroidUnwinderData& data) {";
                    let ucontext_replacement = format!(
                        "{ucontext_signature}\n#if defined(__APPLE__)\n  if (!Initialize(data.error)) return false;\n  return DarwinNativeUnwindUcontext(maps_.get(), jit_debug_.get(), dex_files_.get(), max_frames_, ucontext, data);\n#endif"
                    );
                    let signature = "bool AndroidLocalUnwinder::InternalUnwind(std::optional<pid_t> tid, AndroidUnwinderData& data) {";
                    let replacement = format!(
                        "{signature}\n#if defined(__APPLE__)\n  if (!tid) {{\n    return DarwinNativeUnwind(maps_.get(), jit_debug_.get(), dex_files_.get(), max_frames_, data);\n  }}\n  return DarwinNativeUnwindThread(maps_.get(), jit_debug_.get(), dex_files_.get(), max_frames_, static_cast<uint64_t>(*tid), data);\n#endif"
                    );
                    let remote_signature = "bool AndroidRemoteUnwinder::InternalUnwind(std::optional<pid_t> tid, AndroidUnwinderData& data) {";
                    let remote_init_signature = "bool AndroidRemoteUnwinder::InternalInitialize(ErrorData& error) {";
                    let remote_init_replacement = format!(
                        "{remote_init_signature}\n#if defined(__APPLE__)\n  // Darwin has no Linux ptrace register probe. The ART runtime is ARM64;\n  // defer register acquisition to the Mach task/thread unwinder below.\n  arch_ = ARCH_ARM64;\n#endif"
                    );
                    let remote_replacement = format!(
                        "{remote_signature}\n#if defined(__APPLE__)\n  return DarwinNativeUnwindRemote(maps_.get(), jit_debug_.get(), dex_files_.get(), max_frames_, pid_,\n                                  tid ? static_cast<uint64_t>(*tid) : 0, data);\n#endif"
                    );
                    let lowered = contents
                        .replace(ucontext_signature, &ucontext_replacement)
                        .replace(signature, &replacement)
                        .replace(remote_init_signature, &remote_init_replacement)
                        .replace(remote_signature, &remote_replacement);
                    if !lowered.contains("return DarwinNativeUnwindUcontext")
                        || !lowered.contains("return DarwinNativeUnwindThread")
                        || !lowered.contains("return DarwinNativeUnwindRemote")
                    {
                        return Err("AOSP Darwin AndroidLocalUnwinder lowering contract drift".into());
                    }
                    format!("#include \"darwin_unwindstack_native.h\"\n// aim managed-frame callback bridge.\n{lowered}")
                } else if *name == "Global.cpp" {
                    let signature =
                        "void Global::FindAndReadVariable(Maps* maps, const char* var_str) {";
                    let replacement = format!(
                        "{signature}\n#if defined(__APPLE__)\n  if (uint64_t address = DarwinFindGlobalVariable(maps, var_str); address != 0 &&\n      ReadVariableData(address)) {{\n    return;\n  }}\n  if (void* address = dlsym(RTLD_DEFAULT, var_str); address != nullptr &&\n      ReadVariableData(reinterpret_cast<uint64_t>(address))) {{\n    return;\n  }}\n#endif"
                    );
                    let lowered = contents.replace(signature, &replacement);
                    if !lowered.contains("DarwinFindGlobalVariable(maps, var_str)")
                        || !lowered.contains("dlsym(RTLD_DEFAULT")
                    {
                        return Err("AOSP Darwin global-debug lowering contract drift".into());
                    }
                    format!("#include <dlfcn.h>\n#include \"darwin_unwindstack_native.h\"\n{lowered}")
                } else if *name == "Memory.cpp" {
                    contents.replace(
                        "std::make_optional<pthread_t>()",
                        "std::make_optional<pthread_key_t>()",
                    )
                } else if *name == "MemoryMte.cpp" {
                    contents.replace(
                        "#if defined(__aarch64__)",
                        "#if defined(__aarch64__) && !defined(__APPLE__)",
                    )
                } else {
                    let old = "  arm64_ucontext_t* arm64_ucontext = reinterpret_cast<arm64_ucontext_t*>(ucontext);\n\n  RegsArm64* regs = new RegsArm64();\n  memcpy(regs->RawData(), &arm64_ucontext->uc_mcontext.regs[0], ARM64_REG_LAST * sizeof(uint64_t));\n  return regs;";
                    let new = "#if defined(__APPLE__)\n  auto* context = reinterpret_cast<ucontext_t*>(ucontext);\n  if (context == nullptr || context->uc_mcontext == nullptr) return nullptr;\n  const auto& state = context->uc_mcontext->__ss;\n  RegsArm64* regs = new RegsArm64();\n  auto* raw = reinterpret_cast<uint64_t*>(regs->RawData());\n  for (size_t index = 0; index < 29; ++index) raw[index] = state.__x[index];\n  raw[ARM64_REG_R29] = state.__fp;\n  raw[ARM64_REG_LR] = state.__lr;\n  raw[ARM64_REG_SP] = state.__sp;\n  raw[ARM64_REG_PC] = state.__pc;\n  raw[ARM64_REG_PSTATE] = state.__cpsr;\n  return regs;\n#else\n  arm64_ucontext_t* arm64_ucontext = reinterpret_cast<arm64_ucontext_t*>(ucontext);\n\n  RegsArm64* regs = new RegsArm64();\n  memcpy(regs->RawData(), &arm64_ucontext->uc_mcontext.regs[0], ARM64_REG_LAST * sizeof(uint64_t));\n  return regs;\n#endif";
                    contents.replace(old, new)
                };
                if patched == contents {
                    return Err(
                        format!("AOSP Darwin unwind provider lowering contract drift: {name}")
                            .into(),
                    );
                }
                fs::write(&generated, patched)?;
                generated
            } else {
                upstream_source
            };
            let object = provider_object_dir.join(format!("{}.o", name.replace('/', "_")));
            let mut command = common_cpp_command(&includes);
            command
                .arg("-std=gnu++20")
                .arg("-O2")
                .arg("-DNDEBUG")
                .arg("-D_XOPEN_SOURCE=700");
            command
                .arg("-DDEXFILE_SUPPORT")
                .arg("-DAIM_PREINCLUDED_ELF")
                .arg("-include")
                .arg(ndk_include.join("elf.h"));
            if matches!(*name, "Global.cpp" | "darwin_unwindstack_native.cc") {
                command.arg("-D_DARWIN_C_SOURCE");
            }
            command
                .arg("-include")
                .arg(root.join("compat/darwin_unwindstack_linux_abi.h"))
                .arg("-idirafter")
                .arg(&ndk_arch_include)
                .arg("-idirafter")
                .arg(&ndk_include)
                .arg("-c")
                .arg(source_path)
                .arg("-o")
                .arg(&object);
            Ok(PendingNativeCompile { command, object })
        })
        .collect::<Result<Vec<_>>>()?;
    let (provider_objects, provider_compiled, provider_cached) =
        compile_pending_native(provider_jobs, &compiler_identity)?;
    let provider_archive = build.join("libunwindstack-mach-providers.a");
    create_archive(&provider_archive, &provider_objects)?;

    // The smoke executable must remain independent of the production
    // libdexfile owner while still satisfying the DEX-enabled provider ABI.
    // Build a tiny AOSP-contract stub only for that executable.
    let smoke_stub_source = build.join("dex-files-smoke-stub.cc");
    fs::write(
        &smoke_stub_source,
        "#include <unwindstack/DexFiles.h>\n#include \"DexFile.h\"\nnamespace unwindstack { template <> bool GlobalDebugInterface<DexFile>::Load(Maps*, std::shared_ptr<Memory>&, uint64_t, uint64_t, std::shared_ptr<DexFile>&) { return false; } std::unique_ptr<DexFiles> CreateDexFiles(ArchEnum, std::shared_ptr<Memory>&, std::vector<std::string>) { return nullptr; } }\n",
    )?;
    let smoke_stub_object = build.join("dex-files-smoke-stub.o");
    let mut smoke_stub_command = common_cpp_command(&includes);
    smoke_stub_command
        .arg("-std=gnu++20")
        .arg("-O2")
        .arg("-DNDEBUG")
        .arg("-DAIM_PREINCLUDED_ELF")
        .arg("-include")
        .arg(ndk_include.join("elf.h"))
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-c")
        .arg(&smoke_stub_source)
        .arg("-o")
        .arg(&smoke_stub_object);
    run_command(&mut smoke_stub_command)?;
    let smoke_stub_archive = build.join("libunwindstack-dex-smoke-stub.a");
    create_archive(&smoke_stub_archive, &[smoke_stub_object])?;

    let lzma_archive = build_jit_libelffile(root)?;
    let android_base = root.join("_build/libbase-foundation/libandroid-base-darwin.a");
    let android_log = root.join("_build/graphics-foundations/liblog-darwin.a");
    if !android_base.is_file() {
        return Err(
            "full pinned Android base archive is missing; run `art-bootstrap build-libbase`".into(),
        );
    }
    if !android_log.is_file() {
        return Err(
            "pinned Android log archive is missing; run `art-bootstrap build-graphics-foundations`"
                .into(),
        );
    }
    let smoke = build.join("unwindstack-mach-provider-smoke");
    let mut smoke_link = common_cpp_command(&includes);
    smoke_link
        .arg("-std=gnu++20")
        .arg("-O2")
        .arg("-D_XOPEN_SOURCE=700")
        .arg("-D_DARWIN_C_SOURCE")
        .arg("-idirafter")
        .arg(&ndk_arch_include)
        .arg("-idirafter")
        .arg(&ndk_include)
        .arg(root.join("tools/unwindstack-mach-provider-smoke.cc"))
        .arg(&provider_archive)
        .arg(&archive)
        .arg(&smoke_stub_archive)
        .arg(&rust_demangle_archive)
        .arg(&lzma_archive)
        .arg(&android_base)
        .arg(&android_log)
        .args(["-L/opt/homebrew/lib", "-lzstd", "-lz", "-o"])
        .arg(&smoke);
    run_command(&mut smoke_link)?;
    run_command(
        Command::new("codesign")
            .args(["--force", "--sign", "-", "--entitlements"])
            .arg(root.join("tools/unwindstack-task-access.entitlements.plist"))
            .arg(&smoke),
    )?;
    run_command(&mut Command::new(&smoke))?;
    println!(
        "build-runtime-unwindstack-core: AOSP objects={} compiled={compiled} cached={cached} Darwin providers={} compiled={provider_compiled} cached={provider_cached} archives={},{}",
        objects.len(),
        provider_objects.len(),
        archive.display(),
        provider_archive.display()
    );
    Ok(archive)
}

/// Build the optional AOSP DexFile parser as a separate owner archive.  The
/// portable unwindstack smoke binary intentionally links only the contract
/// stub in `DexFiles.cpp`; production runtime links add this archive together
/// with the real libdexfile/foundation owners.
pub(crate) fn build_runtime_unwindstack_dex(root: &Path) -> Result<PathBuf> {
    let source = root.join("_aosp/system/unwinding/libunwindstack");
    let build = root.join("_build/runtime-unwindstack");
    let object_dir = build.join("dex-objects");
    fs::create_dir_all(&object_dir)?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let includes = [
        root.join("compat"),
        source.join("include"),
        source.clone(),
        root.join("_aosp/art/libartbase"),
        root.join("_aosp/art/libdexfile"),
        root.join("_aosp/art/libdexfile/external/include"),
        root.join("_aosp/system/libbase/include"),
        root.join("_aosp/external/zlib"),
        PathBuf::from("/opt/homebrew/include"),
    ];
    let include_refs: Vec<&Path> = includes.iter().map(PathBuf::as_path).collect();
    let compiler_identity = command_output(Command::new("clang++").arg("--version"))?;
    let sources = [
        ("DexFiles.cpp", source.join("DexFiles.cpp")),
        ("DexFile.cpp", source.join("DexFile.cpp")),
        (
            "dex_file_supp.cc",
            root.join("_aosp/art/libdexfile/external/dex_file_supp.cc"),
        ),
    ];
    let jobs = sources
        .iter()
        .map(|(name, path)| {
            let object = object_dir.join(format!("{name}.o"));
            let mut command = common_cpp_command(&include_refs);
            command
                .arg("-std=gnu++20")
                .arg("-O2")
                .arg("-DNDEBUG")
                .arg("-DDEXFILE_SUPPORT")
                .arg("-DSTATIC_LIB")
                .arg("-DAIM_PREINCLUDED_ELF")
                .arg("-include")
                .arg(ndk_include.join("elf.h"))
                .arg("-idirafter")
                .arg(&ndk_include)
                .arg("-idirafter")
                .arg(&ndk_arch_include)
                .arg("-c")
                .arg(path)
                .arg("-o")
                .arg(&object);
            PendingNativeCompile { command, object }
        })
        .collect::<Vec<_>>();
    let (objects, _, _) = compile_pending_native(jobs, &compiler_identity)?;
    let archive = build.join("libunwindstack-dex-darwin.a");
    create_archive(&archive, &objects)?;
    Ok(archive)
}

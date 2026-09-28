use super::*;
use crate::native_build::{PendingNativeCompile, compile_pending_native};

fn source_list(block: &str) -> Result<Vec<String>> {
    let start = block.find("srcs: [").ok_or("missing JIT source list")? + "srcs: [".len();
    let end = block[start..]
        .find(']')
        .ok_or("unterminated JIT source list")?
        + start;
    let sources: Vec<_> = block[start..end]
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.strip_prefix('"')
                .and_then(|s| s.split('"').next())
                .map(str::to_owned)
        })
        .collect();
    if sources.is_empty()
        || sources
            .iter()
            .any(|s| !s.ends_with(".cc") || s.contains("..") || s.contains('*'))
    {
        return Err("unexpected pinned JIT source list".into());
    }
    Ok(sources)
}

fn audit_implicit_null_checks(jit_compiler: &str, arm64_codegen: &str) -> Result<()> {
    const RUNTIME_SETTING: &str =
        "compiler_options_->implicit_null_checks_ = runtime->GetImplicitNullChecks();";
    if !jit_compiler.contains(RUNTIME_SETTING) {
        return Err(
            "patched JIT compiler no longer inherits ART's implicit-null-check setting".into(),
        );
    }
    if jit_compiler.contains("compiler_options_->implicit_null_checks_ = false;") {
        return Err(
            "patched JIT compiler still disables AOSP implicit null checks on Darwin".into(),
        );
    }

    // Darwin compressed references need a nullable decode before the faulting ARM64 load.
    // Keep the three consumers that can observe a null receiver covered together: an
    // explicit HNullCheck, instance fields, and interface/virtual invokes.
    for marker in [
        "void CodeGeneratorARM64::GenerateImplicitNullCheck(HNullCheck* instruction)",
        "void InstructionCodeGeneratorARM64::HandleFieldGet(HInstruction* instruction",
        "void InstructionCodeGeneratorARM64::VisitInvokeInterface(HInvokeInterface* invoke)",
        "void CodeGeneratorARM64::GenerateVirtualCall(",
    ] {
        let start = arm64_codegen
            .find(marker)
            .ok_or_else(|| format!("missing ARM64 implicit-null consumer: {marker}"))?;
        let body = &arm64_codegen[start..arm64_codegen.len().min(start + 4_096)];
        if !body.contains("DecodeNullable") {
            return Err(
                format!("ARM64 implicit-null consumer lacks nullable decode: {marker}").into(),
            );
        }
    }
    Ok(())
}

fn audit_compiled_jni_transitions(jni_compiler: &str) -> Result<()> {
    let native_start = jni_compiler
        .find("std::unique_ptr<JNIMacroLabel> transition_to_native_slow_path;")
        .ok_or("compiled JNI native-transition block is missing")?;
    let native_end = jni_compiler[native_start..]
        .find("// 3. Push local reference frame.")
        .ok_or("compiled JNI native-transition block end is missing")?
        + native_start;
    let native_fast = &jni_compiler[native_start..native_end];
    if !native_fast.contains("TryToTransitionFromRunnableToNative(")
        || native_fast.contains("#if defined(__APPLE__)")
        || native_fast.contains("pJniMethodStart")
    {
        return Err("Darwin compiled JNI no longer uses AOSP's inline native transition".into());
    }

    let runnable_start = jni_compiler
        .find("std::unique_ptr<JNIMacroLabel> transition_to_runnable_slow_path;")
        .ok_or("compiled JNI runnable-transition block is missing")?;
    let runnable_end = jni_compiler[runnable_start..]
        .find("// 5.2. For methods that return a reference")
        .ok_or("compiled JNI runnable-transition block end is missing")?
        + runnable_start;
    let runnable_fast = &jni_compiler[runnable_start..runnable_end];
    if !runnable_fast.contains("TryToTransitionFromNativeToRunnable(")
        || runnable_fast.contains("#if defined(__APPLE__)")
        || runnable_fast.contains("pJniMethodEnd")
    {
        return Err("Darwin compiled JNI no longer uses AOSP's inline runnable transition".into());
    }

    let slow_start = jni_compiler
        .find("// 8.2. Slow path for transition to Native.")
        .ok_or("compiled JNI transition slow paths are missing")?;
    let slow_end = jni_compiler[slow_start..]
        .find("// 8.4. Exception poll slow path(s).")
        .ok_or("compiled JNI transition slow-path end is missing")?
        + slow_start;
    let slow_paths = &jni_compiler[slow_start..slow_end];
    if !slow_paths.contains("__ Bind(transition_to_native_slow_path.get());")
        || !slow_paths.contains("pJniMethodStart")
        || !slow_paths.contains("__ Bind(transition_to_runnable_slow_path.get());")
        || !slow_paths.contains("pJniMethodEnd")
        || slow_paths.contains("#if !defined(__APPLE__)")
    {
        return Err("Darwin compiled JNI AOSP transition slow paths are incomplete".into());
    }
    Ok(())
}

pub(crate) fn build_jit_compiler(root: &Path) -> Result<()> {
    build_jit_libelffile(root)?;
    let compiler = root.join("_aosp/art/compiler");
    let bp = fs::read_to_string(compiler.join("Android.bp"))?;
    let defaults = bp
        .split("name: \"libart-compiler-defaults\"")
        .nth(1)
        .ok_or("missing compiler defaults; run sync-jit-sources")?;
    let mut sources = source_list(defaults)?;
    let arm64 = defaults
        .split("arm64: {")
        .nth(1)
        .ok_or("missing ARM64 compiler sources")?;
    sources.extend(source_list(arm64)?);
    // AOSP's ARM/ARM64 builds share these passes through the ARM defaults.
    sources.extend([
        "optimizing/instruction_simplifier_shared.cc".to_owned(),
        "optimizing/nodes_shared.cc".to_owned(),
    ]);
    let build = root.join("_build/jit-compiler");
    fs::create_dir_all(build.join("objects"))?;
    let staged_jit = build.join("patched-source/compiler/jit");
    let staged_jni_quick = build.join("patched-source/compiler/jni/quick");
    let staged_jni_arm64 = staged_jni_quick.join("arm64");
    let staged_jni_utils = build.join("patched-source/compiler/utils");
    let staged_jni_assembler_arm64 = build.join("patched-source/compiler/utils/arm64");
    let staged_codegen = build.join("patched-source/compiler/optimizing");
    fs::create_dir_all(&staged_jni_arm64)?;
    fs::create_dir_all(&staged_jni_utils)?;
    fs::create_dir_all(&staged_jni_assembler_arm64)?;
    fs::create_dir_all(&staged_codegen)?;
    fs::copy(
        compiler.join("optimizing/code_generator_arm64.cc"),
        staged_codegen.join("code_generator_arm64.cc"),
    )?;
    fs::copy(
        compiler.join("optimizing/code_generator_arm64.h"),
        staged_codegen.join("code_generator_arm64.h"),
    )?;
    fs::copy(
        compiler.join("optimizing/jit_patches_arm64.cc"),
        staged_codegen.join("jit_patches_arm64.cc"),
    )?;
    fs::copy(
        compiler.join("optimizing/jit_patches_arm64.h"),
        staged_codegen.join("jit_patches_arm64.h"),
    )?;
    // These TUs include code_generator_arm64.h from their own source
    // directory, which takes precedence over -I staging paths. Keep the
    // patched wrapper ABI consistent across every production consumer.
    for source in [
        "code_generator.cc",
        "code_generator_vector_arm64_neon.cc",
        "code_generator_vector_arm64_sve.cc",
        "intrinsics_arm64.cc",
        "fast_compiler_arm64.cc",
        "inliner.cc",
        "instruction_simplifier_arm64.cc",
    ] {
        fs::copy(
            compiler.join("optimizing").join(source),
            staged_codegen.join(source),
        )?;
    }
    fs::copy(
        compiler.join("optimizing/optimizing_compiler.cc"),
        staged_codegen.join("optimizing_compiler.cc"),
    )?;
    fs::copy(
        compiler.join("optimizing/sharpening.cc"),
        staged_codegen.join("sharpening.cc"),
    )?;
    fs::create_dir_all(&staged_jit)?;
    fs::copy(
        compiler.join("jit/jit_compiler.cc"),
        staged_jit.join("jit_compiler.cc"),
    )?;
    fs::copy(
        compiler.join("jni/quick/jni_compiler.cc"),
        staged_jni_quick.join("jni_compiler.cc"),
    )?;
    fs::copy(
        compiler.join("jni/quick/calling_convention.cc"),
        staged_jni_quick.join("calling_convention.cc"),
    )?;
    fs::copy(
        compiler.join("jni/quick/arm64/calling_convention_arm64.cc"),
        staged_jni_arm64.join("calling_convention_arm64.cc"),
    )?;
    fs::copy(
        compiler.join("jni/quick/arm64/calling_convention_arm64.h"),
        staged_jni_arm64.join("calling_convention_arm64.h"),
    )?;
    fs::copy(
        compiler.join("utils/arm64/jni_macro_assembler_arm64.cc"),
        staged_jni_assembler_arm64.join("jni_macro_assembler_arm64.cc"),
    )?;
    fs::copy(
        compiler.join("utils/jni_macro_assembler.h"),
        staged_jni_utils.join("jni_macro_assembler.h"),
    )?;
    fs::copy(
        compiler.join("utils/managed_register.h"),
        staged_jni_utils.join("managed_register.h"),
    )?;
    fs::copy(
        compiler.join("utils/arm64/jni_macro_assembler_arm64.h"),
        staged_jni_assembler_arm64.join("jni_macro_assembler_arm64.h"),
    )?;
    fs::copy(
        compiler.join("utils/arm64/assembler_arm64.h"),
        staged_jni_assembler_arm64.join("assembler_arm64.h"),
    )?;
    fs::copy(
        compiler.join("utils/arm64/managed_register_arm64.h"),
        staged_jni_assembler_arm64.join("managed_register_arm64.h"),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0043-darwin-jit-reference-return.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0095-darwin-arm64-jni-handle-return.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0121-darwin-arm64-jni-monitor-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0096-darwin-arm64-string-intrinsic-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0097-darwin-arm64-crc32-array-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0098-darwin-arm64-reference-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0122-darwin-arm64-image-method-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0099-darwin-arm64-vector-memory-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0040-darwin-jit-compiler-gate.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0044-darwin-arm64-compressed32-field-get.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0045-darwin-jit-forwarding-call-abi.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0050-darwin-jit-native-root-slot-literals.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0082-darwin-fast-jit-native-root-slot-literals.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0052-darwin-jit-boot-object-literals.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(
                root.join(
                    "patches/art/0053-darwin-arm64-compressed-field-store-card-address.patch",
                ),
            )
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0054-darwin-arm64-compressed-array-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0060-darwin-jit-inline-capability.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0061-darwin-arm64-throw-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0062-darwin-arm64-monitor-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0063-darwin-arm64-type-check-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    let operator_source = build.join("operator_out.cc");
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0064-darwin-arm64-interface-check-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0065-darwin-arm64-array-type-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0068-darwin-string-resolution-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0069-darwin-interface-dispatch-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0056-darwin-arm64-allocation-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    let mut generator = Command::new("python3");
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0073-darwin-polymorphic-runtime-dispatch.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0075-darwin-varhandle-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0077-darwin-varhandle-fp-acquire-scratch.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0071-darwin-unresolved-static-field-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0059-darwin-arm64-virtual-dispatch-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0079-darwin-baker-reference-window.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0080-darwin-baker-array-reference-window.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0083-darwin-reference-array-intermediate-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0084-darwin-baker-unresolved-fields.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0085-darwin-baker-gc-root-thunk-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0087-darwin-baker-unresolved-invokes.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0088-darwin-invoke-custom-graph.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0089-darwin-aosp-arm64-intrinsics.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0090-darwin-unsafe-get-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0091-darwin-unsafe-write-atomic-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0093-darwin-aosp-jit-admission.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0124-darwin-arm64-implicit-null-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0135-darwin-remove-optimizing-allowlist.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0136-darwin-remove-optimizing-allowlist-tail.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0127-darwin-arm64-boxing-allocation-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0129-darwin-arm64-baker-intermediate-array-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0131-darwin-arm64-boxing-cache-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0133-darwin-arm64-implicit-invoke-receiver.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0134-darwin-arm64-implicit-field-receiver.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0147-darwin-enable-implicit-null-checks.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0135-darwin-arm64-jni-stack-abi.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0145-darwin-arm64-jni-method-pointer.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0146-darwin-arm64-managed-method-pointer.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0136-darwin-arm64-char-arraycopy-addresses.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0137-darwin-arm64-frame-clinit-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0138-darwin-sharpening-boot-image-address.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0139-darwin-arm64-boot-literal-reference.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0140-darwin-arm64-reference-intrinsic-class.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0179-darwin-arm64-clinit-reference-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0180-darwin-arm64-fast-invoke-receiver-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0181-darwin-fast-reference-codegen-include.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0182-darwin-arm64-fast-field-reference-boundaries.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0183-darwin-arm64-fast-checkcast-reference-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0184-darwin-arm64-reference-referent-boundary.patch"))
            .current_dir(build.join("patched-source")),
    )?;
    audit_implicit_null_checks(
        &fs::read_to_string(staged_jit.join("jit_compiler.cc"))?,
        &fs::read_to_string(staged_codegen.join("code_generator_arm64.cc"))?,
    )?;
    audit_compiled_jni_transitions(&fs::read_to_string(
        staged_jni_quick.join("jni_compiler.cc"),
    )?)?;
    generator
        .arg(root.join("_aosp/art/tools/generate_operator_out.py"))
        .arg(&compiler);
    for header in [
        "linker/linker_patch.h",
        "optimizing/locations.h",
        "optimizing/nodes.h",
        "optimizing/optimizing_compiler_stats.h",
        "utils/arm/constants_arm.h",
        "optimizing/nodes_shared.h",
    ] {
        generator.arg(compiler.join(header));
    }
    let generated = generator.output()?;
    if !generated.status.success() {
        return Err(String::from_utf8_lossy(&generated.stderr)
            .into_owned()
            .into());
    }
    fs::write(&operator_source, &generated.stdout)?;
    let include_paths = [
        "_build/jit-compiler/patched-source/compiler",
        "_build/jit-compiler/patched-source/compiler/optimizing",
        "_build/runtime-common/patched-source/runtime",
        "_build/foundation/patched-source/libartbase",
        "_build/runtime-arm64/generated",
        "compat",
        "_aosp/art/compiler",
        "_aosp/art/compiler/jni/quick",
        "_aosp/art/compiler/jni/quick/arm64",
        "_aosp/art/compiler/jit",
        "_aosp/art/compiler/optimizing",
        "_aosp/art/runtime",
        "_aosp/art/runtime/jit",
        "_aosp/art/runtime/base",
        "_aosp/art/libartbase",
        "_aosp/art/libdexfile",
        "_aosp/art/libprofile",
        "_aosp/art/libelffile",
        "_aosp/art",
        "_aosp/art/cmdline",
        "_aosp/art/disassembler",
        "_aosp/art/tools/cpp-define-generator",
        "_aosp/art/libartpalette/include",
        "_aosp/system/libbase/include",
        "_aosp/libnativehelper/include_jni",
        "_aosp/libnativehelper/header_only_include",
        "_aosp/external/vixl/src",
        "_aosp/external/vixl",
        "_aosp/external/dlmalloc",
        "_aosp/external/tinyxml2",
        "_aosp/external/fmtlib/include",
    ]
    .map(|p| root.join(p));
    let mut includes: Vec<&Path> = include_paths.iter().map(PathBuf::as_path).collect();
    includes.push(Path::new("/opt/homebrew/include"));
    let identity = command_output(Command::new("clang++").arg("--version"))?;
    let (ndk_include, ndk_arch_include) = find_ndk_headers()?;
    let mut jobs = Vec::new();
    let mut source_paths: Vec<_> = sources.into_iter().map(|s| compiler.join(s)).collect();
    source_paths.push(operator_source);
    source_paths.push(root.join("_aosp/art/disassembler/disassembler.cc"));
    source_paths.push(root.join("_aosp/art/disassembler/disassembler_arm64.cc"));
    source_paths.push(root.join("_aosp/art/runtime/arch/arm64/registers_arm64.cc"));
    for source in [
        "arm/instruction_set_features_arm.cc",
        "x86/instruction_set_features_x86.cc",
        "riscv64/instruction_set_features_riscv64.cc",
    ] {
        source_paths.push(root.join("_aosp/art/runtime/arch").join(source));
    }
    for directory in ["_aosp/external/vixl/src", "_aosp/external/vixl/src/aarch64"] {
        for entry in fs::read_dir(root.join(directory))? {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "cc") {
                source_paths.push(path);
            }
        }
    }
    source_paths.sort();
    for source in source_paths {
        let relative = source.strip_prefix(root)?;
        let object = build.join("objects").join(format!(
            "{}.o",
            relative.to_string_lossy().replace('/', "_")
        ));
        let mut command = runtime_cpp_command(&includes);
        command.args([
            "-std=gnu++20",
            "-DART_ENABLE_CODEGEN_arm64",
            "-DVIXL_INCLUDE_TARGET_A64",
            "-DVIXL_INCLUDE_SIMULATOR_AARCH64",
            "-DVIXL_GENERATE_SIMULATOR_INSTRUCTIONS_VALUE=0",
            "-DVIXL_CODE_BUFFER_MALLOC",
            "-Wno-unused-command-line-argument",
            "-include",
            "mirror/object_reference.h",
        ]);
        // Keep the compiler's JitMemoryRegion layout identical to the runtime,
        // including Darwin's MAP_JIT state, even through relative includes.
        if source.starts_with(&compiler) {
            command.arg("-include").arg(
                root.join("_build/runtime-common/patched-source/runtime/jit/jit_memory_region.h"),
            );
            // oat_file.h includes index_bss_mapping.h with AOSP quote
            // semantics; keep the immutable runtime/oat sibling directory in
            // the compiler's fallback search path.
            command
                .arg("-iquote")
                .arg(root.join("_aosp/art/runtime/oat"));
        }
        command
            .arg("-idirafter")
            .arg(&ndk_include)
            .arg("-idirafter")
            .arg(&ndk_arch_include);
        let source = if source == compiler.join("jni/quick/jni_compiler.cc") {
            staged_jni_quick.join("jni_compiler.cc")
        } else if source == compiler.join("jni/quick/calling_convention.cc") {
            staged_jni_quick.join("calling_convention.cc")
        } else if source == compiler.join("jni/quick/arm64/calling_convention_arm64.cc") {
            staged_jni_arm64.join("calling_convention_arm64.cc")
        } else if source == compiler.join("utils/arm64/jni_macro_assembler_arm64.cc") {
            staged_jni_assembler_arm64.join("jni_macro_assembler_arm64.cc")
        } else if source == compiler.join("jit/jit_compiler.cc") {
            staged_jit.join("jit_compiler.cc")
        } else if source == compiler.join("optimizing/code_generator.cc") {
            staged_codegen.join("code_generator.cc")
        } else if source == compiler.join("optimizing/code_generator_arm64.cc") {
            staged_codegen.join("code_generator_arm64.cc")
        } else if source == compiler.join("optimizing/code_generator_vector_arm64_neon.cc") {
            staged_codegen.join("code_generator_vector_arm64_neon.cc")
        } else if source == compiler.join("optimizing/code_generator_vector_arm64_sve.cc") {
            staged_codegen.join("code_generator_vector_arm64_sve.cc")
        } else if source == compiler.join("optimizing/intrinsics_arm64.cc") {
            staged_codegen.join("intrinsics_arm64.cc")
        } else if source == compiler.join("optimizing/fast_compiler_arm64.cc") {
            staged_codegen.join("fast_compiler_arm64.cc")
        } else if source == compiler.join("optimizing/jit_patches_arm64.cc") {
            staged_codegen.join("jit_patches_arm64.cc")
        } else if source == compiler.join("optimizing/inliner.cc") {
            staged_codegen.join("inliner.cc")
        } else if source == compiler.join("optimizing/instruction_simplifier_arm64.cc") {
            staged_codegen.join("instruction_simplifier_arm64.cc")
        } else if source == compiler.join("optimizing/optimizing_compiler.cc") {
            staged_codegen.join("optimizing_compiler.cc")
        } else if source == compiler.join("optimizing/sharpening.cc") {
            staged_codegen.join("sharpening.cc")
        } else {
            source
        };
        command.arg("-c").arg(source).arg("-o").arg(&object);
        jobs.push(PendingNativeCompile { command, object });
    }
    let (objects, compiled, cached) = compile_pending_native(jobs, &identity)?;
    let archive = build.join("libart-compiler-darwin.a");
    create_archive(&archive, &objects)?;
    println!(
        "build-jit-compiler: ARM64 AOSP objects={} compiled={compiled} cached={cached} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(())
}

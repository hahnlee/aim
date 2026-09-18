use super::*;

fn restore_apple_dwarf_expression_macros(source: &str) -> Result<String> {
    let expression = r#".macro CFI_EXPRESSION_BREG n, b, offset
    .if (-0x40 <= (\offset)) && ((\offset) < 0x40)
        .cfi_escape 0x10, \n, 2, 0x70+\b, (\offset) & 0x7f
    .elseif (-0x2000 <= (\offset)) && ((\offset) < 0x2000)
        .cfi_escape 0x10, \n, 3, 0x70+\b, ((\offset) & 0x7f) | 0x80, ((\offset) >> 7) & 0x7f
    .else
        .error "Unsupported offset"
    .endif
.endm
"#;
    let cfa_expression = r#".macro CFI_DEF_CFA_BREG_PLUS_UCONST reg, offset, size
    .if ((\size) < 0)
        .error "Size should be positive"
    .endif
    .if (((\offset) < -0x40) || ((\offset) >= 0x40))
        .error "Unsupported offset"
    .endif
    .if ((\size) < 0x80)
        .cfi_escape 0x0f, 6, 0x92, \reg, (\offset) & 0x7f, 0x06, 0x23, \size
    .elseif ((\size) < 0x4000)
        .cfi_escape 0x0f, 7, 0x92, \reg, (\offset) & 0x7f, 0x06, 0x23, ((\size) & 0x7f) | 0x80, ((\size) >> 7) & 0x7f
    .else
        .error "Unsupported size"
    .endif
.endm
"#;
    let mut restored = String::with_capacity(source.len() + 1024);
    let mut lines = source.lines();
    let mut replacements = 0usize;
    while let Some(line) = lines.next() {
        let replacement = match line.trim() {
            ".macro CFI_EXPRESSION_BREG n, b, offset" => Some(expression),
            ".macro CFI_DEF_CFA_BREG_PLUS_UCONST reg, offset, size" => Some(cfa_expression),
            _ => None,
        };
        if let Some(replacement) = replacement {
            restored.push_str(replacement);
            replacements += 1;
            for skipped in lines.by_ref() {
                if skipped.trim() == ".endm" {
                    break;
                }
            }
        } else {
            restored.push_str(line);
            restored.push('\n');
        }
    }
    if replacements != 2 {
        return Err(format!(
            "Darwin CFI expression macro inventory drift: restored {replacements}/2"
        )
        .into());
    }
    Ok(restored)
}

const CRITICAL_REFS_ARGS_EXPRESSION_SEED: &str = "\
\t.cfi_def_cfa sp, 224\n\
\t.cfi_escape 0x10, 20, 3, 0x8d, 0x88, 0x01\n\
\t.cfi_escape 0x10, 21, 3, 0x8d, 0x90, 0x01\n\
\t.cfi_escape 0x10, 22, 3, 0x8d, 0x98, 0x01\n\
\t.cfi_escape 0x10, 23, 3, 0x8d, 0xa0, 0x01\n\
\t.cfi_escape 0x10, 24, 3, 0x8d, 0xa8, 0x01\n\
\t.cfi_escape 0x10, 25, 3, 0x8d, 0xb0, 0x01\n\
\t.cfi_escape 0x10, 26, 3, 0x8d, 0xb8, 0x01\n\
\t.cfi_escape 0x10, 27, 3, 0x8d, 0xc0, 0x01\n\
\t.cfi_escape 0x10, 28, 3, 0x8d, 0xc8, 0x01\n\
\t.cfi_escape 0x10, 29, 3, 0x8d, 0xd0, 0x01\n\
\t.cfi_escape 0x10, 30, 2, 0x8d, 0x08\n";

fn lower_dlsym_cfi(assembly: &str, source: &str) -> Result<String> {
    let mut lowered = String::with_capacity(source.len() + 1024);
    let mut splits = 0usize;
    let mut skipped_state_ops = 0usize;
    let mut critical_frame_moves = 0usize;
    for line in source.lines() {
        let trimmed = line.trim();
        if assembly == "jni_entrypoints_arm64.S"
            && matches!(trimmed, ".cfi_remember_state" | ".cfi_restore_state")
        {
            // The normal and exception continuations become independent FDEs
            // with explicit entry state, so no DWARF state stack crosses them.
            skipped_state_ops += 1;
            continue;
        }
        if assembly == "jni_entrypoints_arm64.S" && trimmed == "mov\tx29, x13" {
            let cfa = if critical_frame_moves == 0 { 224 } else { 176 };
            lowered.push_str("\t.cfi_endproc\n\t.cfi_startproc\n");
            lowered.push_str(&format!("\t.cfi_def_cfa sp, {cfa}\n"));
            lowered.push_str(line);
            lowered.push('\n');
            critical_frame_moves += 1;
            splits += 1;
            continue;
        }
        let seed = match (assembly, trimmed) {
            ("native_entrypoints_arm64.S", ".Llookup_stub_continue:") => Some(
                "\t.cfi_def_cfa sp, 144\n\
                 \t.cfi_rel_offset x29, 128\n\
                 \t.cfi_rel_offset x30, 136\n",
            ),
            ("jni_entrypoints_arm64.S", ".Lcritical_not_generic_jni:") => Some(""),
            ("jni_entrypoints_arm64.S", ".Lcritical_skip_prepare_runtime_method:") => {
                Some(CRITICAL_REFS_ARGS_EXPRESSION_SEED)
            }
            ("jni_entrypoints_arm64.S", ".Lcritical_skip_copy_args_back:")
            | ("jni_entrypoints_arm64.S", ".Lcritical_deliver_exception:") => {
                Some("\t.cfi_def_cfa sp, 224\n")
            }
            _ => None,
        };
        if let Some(seed) = seed {
            lowered.push_str("\t.cfi_endproc\n");
            lowered.push_str(line);
            lowered.push('\n');
            lowered.push_str("\t.cfi_startproc\n");
            lowered.push_str(seed);
            splits += 1;
        } else {
            lowered.push_str(line);
            lowered.push('\n');
        }
    }
    let expected_splits = if assembly == "native_entrypoints_arm64.S" {
        1
    } else {
        6
    };
    if splits != expected_splits {
        return Err(format!(
            "{assembly} Mach-O dlsym FDE inventory drift: {splits}/{expected_splits}"
        )
        .into());
    }
    if assembly == "jni_entrypoints_arm64.S" {
        if critical_frame_moves != 2 {
            return Err(format!(
                "critical dlsym dynamic-frame inventory drift: {critical_frame_moves}/2"
            )
            .into());
        }
        if skipped_state_ops != 2 {
            return Err(
                format!("critical dlsym CFI state inventory drift: {skipped_state_ops}/2").into(),
            );
        }
        let expression_rules = lowered.matches(".cfi_escape 0x10").count();
        if expression_rules != 34 {
            return Err(format!(
                "critical dlsym register-expression inventory drift: {expression_rules}/34"
            )
            .into());
        }
    }
    Ok(lowered)
}

pub(crate) fn build_runtime_arm64(root: &Path) -> Result<()> {
    let artbase = root.join("_aosp/art/libartbase");
    let patched_artbase = root.join("_build/foundation/patched-source/libartbase");
    let libdexfile = root.join("_aosp/art/libdexfile");
    let runtime = root.join("_aosp/art/runtime");
    let runtime_abi = root.join("_build/runtime-common/patched-source/runtime");
    let runtime_base = runtime.join("base");
    let runtime_arm64 = runtime.join("arch/arm64");
    let generator = root.join("_aosp/art/tools/cpp-define-generator");
    let palette_include = root.join("_aosp/art/libartpalette/include");
    let libbase_include = root.join("_aosp/system/libbase/include");
    let tinyxml2 = root.join("_aosp/external/tinyxml2");
    let android_jni_include = root.join("_aosp/libnativehelper/include_jni");
    let dlmalloc = root.join("_aosp/external/dlmalloc");
    let compat = root.join("compat");
    if !generator.join("asm_defines.cc").exists() {
        return Err("ART ABI generator is missing; run `art-bootstrap sync` first".into());
    }

    let build_dir = root.join("_build/runtime-arm64");
    let generated_dir = build_dir.join("generated");
    let patched_runtime = build_dir.join("patched-source/runtime");
    let patched_arm64 = patched_runtime.join("arch/arm64");
    let object_dir = build_dir.join("objects");
    fs::create_dir_all(&generated_dir)?;
    fs::create_dir_all(&patched_arm64)?;
    fs::create_dir_all(&object_dir)?;
    fs::copy(
        runtime_arm64.join("context_arm64.h"),
        patched_arm64.join("context_arm64.h"),
    )?;
    fs::copy(
        runtime_arm64.join("context_arm64.cc"),
        patched_arm64.join("context_arm64.cc"),
    )?;
    fs::copy(
        runtime_arm64.join("instruction_set_features_arm64.cc"),
        patched_arm64.join("instruction_set_features_arm64.cc"),
    )?;
    for assembly in [
        "asm_support_arm64.S",
        "jni_entrypoints_arm64.S",
        "memcmp16_arm64.S",
        "quick_entrypoints_arm64.S",
        "native_entrypoints_arm64.S",
    ] {
        fs::copy(runtime_arm64.join(assembly), patched_arm64.join(assembly))?;
    }
    for patch in [
        "patches/art/0001-arm64-mach-o-assembly.patch",
        "patches/art/0005-darwin-arm64-context-word-type.patch",
        "patches/art/0012-darwin-arm64-quick-symbols.patch",
        "patches/art/0015-darwin-arm64-feature-fallback.patch",
        "patches/art/0016-darwin-arm64-direct-c-symbols.patch",
        "patches/art/0047-darwin-managed-return-arm64.patch",
        "patches/art/0055-darwin-arm64-aput-object-runtime.patch",
        "patches/art/0058-darwin-arm64-array-component-address.patch",
        "patches/art/0081-darwin-baker-mark-introspection-references.patch",
        "patches/art/0086-darwin-baker-mark-entrypoint-addresses.patch",
        "patches/art/0142-darwin-arm64-generic-jni-tag-handoff.patch",
        "patches/art/0147-darwin-arm64-runtime-method-pointer-boundaries.patch",
        "patches/art/0192-darwin-native-client-public-entrypoints-arm64.patch",
    ] {
        run_command(
            Command::new("patch")
                .args(["--batch", "--forward", "-p1", "-i"])
                .arg(root.join(patch))
                .current_dir(build_dir.join("patched-source")),
        )?;
    }

    let includes = [
        generated_dir.as_path(),
        compat.as_path(),
        generator.as_path(),
        patched_runtime.as_path(),
        runtime_abi.as_path(),
        patched_artbase.as_path(),
        artbase.as_path(),
        libdexfile.as_path(),
        runtime.as_path(),
        runtime_base.as_path(),
        runtime_arm64.as_path(),
        palette_include.as_path(),
        libbase_include.as_path(),
        tinyxml2.as_path(),
        android_jni_include.as_path(),
        dlmalloc.as_path(),
        Path::new("/opt/homebrew/include"),
    ];

    let generated_assembly = generated_dir.join("asm_defines.s");
    run_command(
        runtime_cpp_command(&includes)
            .args(["-include", "mirror/object_reference.h"])
            .arg("-S")
            .arg(compat.join("art_reference_asm_defines.cc"))
            .arg("-o")
            .arg(&generated_assembly),
    )?;
    let generated_header = command_output(
        Command::new("python3")
            .arg(generator.join("make_header.py"))
            .arg(&generated_assembly),
    )?;
    for required in [
        "#define THREAD_FLAGS_OFFSET",
        "#define THREAD_CARD_TABLE_OFFSET",
        "#define THREAD_EXCEPTION_OFFSET",
        "#define THREAD_ID_OFFSET",
        "#define DARWIN_ART_REFERENCE_BASE",
    ] {
        if !generated_header.contains(required) {
            return Err(format!("generated asm_defines.h is missing {required}").into());
        }
    }
    fs::write(generated_dir.join("asm_defines.h"), generated_header)?;

    let sources = [
        patched_arm64.join("context_arm64.cc"),
        runtime.join("arch/context.cc"),
        runtime.join("arch/instruction_set_features.cc"),
        runtime_arm64.join("thread_arm64.cc"),
        runtime_arm64.join("entrypoints_init_arm64.cc"),
        patched_arm64.join("instruction_set_features_arm64.cc"),
    ];
    let mut objects = Vec::new();
    for source in sources {
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("source has no file name: {}", source.display()))?;
        let object = object_dir.join(format!("{}.o", file_name.to_string_lossy()));
        run_command(
            runtime_cpp_command(&includes)
                .args(["-include", "mirror/object_reference.h"])
                .arg("-Wno-deprecated-anon-enum-enum-conversion")
                .arg("-c")
                .arg(&source)
                .arg("-o")
                .arg(&object),
        )?;
        let kind = command_output(Command::new("file").arg(&object))?;
        if !kind.contains("Mach-O 64-bit object arm64") {
            return Err(format!("unexpected ARM64 runtime object format: {kind}").into());
        }
        objects.push(object);
    }

    // Apple's integrated assembler rejects a few non-linear ART CFI programs.
    // Keep the upstream unwind state, but lower control-flow joins to adjacent
    // Mach-O FDEs. This is metadata-only: the instruction stream must remain
    // byte-identical to the AOSP assembly.
    for assembly in [
        "jni_entrypoints_arm64.S",
        "memcmp16_arm64.S",
        "quick_entrypoints_arm64.S",
        "native_entrypoints_arm64.S",
    ] {
        let source = patched_arm64.join(assembly);
        let preprocessed = command_output(
            Command::new("clang")
                .args([
                    "-E",
                    "-x",
                    "assembler-with-cpp",
                    "-DART_PAGE_SIZE_AGNOSTIC",
                    "-DART_USE_READ_BARRIER",
                    "-DART_READ_BARRIER_TYPE_IS_BAKER=1",
                    "-DART_FORCE_USE_READ_BARRIER",
                ])
                .arg(format!("-I{}", generated_dir.display()))
                .arg(format!("-I{}", patched_arm64.display()))
                .arg(format!("-I{}", runtime_arm64.display()))
                .arg(format!("-I{}", runtime.display()))
                .arg(&source),
        )?;
        let mut compile_variant = |variant: &str, contents: &str| -> Result<()> {
            let generated_source = generated_dir.join(format!("{assembly}.darwin-{variant}.S"));
            fs::write(&generated_source, contents)?;
            let object = object_dir.join(format!("{assembly}.{variant}.o"));
            run_command(
                Command::new("clang")
                    .args(["-arch", "arm64", "-x", "assembler", "-c"])
                    .arg(&generated_source)
                    .arg("-o")
                    .arg(&object),
            )?;
            let kind = command_output(Command::new("file").arg(&object))?;
            if !kind.contains("Mach-O 64-bit object arm64") {
                return Err(format!("unexpected ARM64 assembly object format: {kind}").into());
            }
            objects.push(object);
            Ok(())
        };

        if assembly == "quick_entrypoints_arm64.S" {
            // Apple's assembler can emit ART's ordinary quick-entrypoint CFI
            // once assembler macros have been expanded. Lower non-linear ELF
            // CFI state machines to adjacent Mach-O FDEs; retain unwind rules
            // for every quick entrypoint and never emit partial/false CFI.
            let macro_source = generated_dir.join(format!("{assembly}.darwin-macro-cfi.S"));
            let normalized_source = generated_dir.join(format!("{assembly}.darwin-expanded-cfi.S"));
            fs::write(&macro_source, &preprocessed)?;
            run_command(
                Command::new("clang")
                    .args([
                        "-cc1as",
                        "-triple",
                        "arm64-apple-macosx",
                        "-filetype",
                        "asm",
                        "-o",
                    ])
                    .arg(&normalized_source)
                    .arg(&macro_source),
            )?;
            let normalized = fs::read_to_string(&normalized_source)?;
            let mut transformed = String::with_capacity(normalized.len());
            let mut current_entrypoint = String::new();
            let mut retained_cfi_start = 0usize;
            let mut split_slow_paths = 0usize;
            let mut split_control_paths = 0usize;
            let save_everything_seed = "\t.cfi_def_cfa sp, 512\n\
                \t.cfi_rel_offset x0, 272\n\
                \t.cfi_rel_offset x1, 280\n\
                \t.cfi_rel_offset x2, 288\n\
                \t.cfi_rel_offset x3, 296\n\
                \t.cfi_rel_offset x4, 304\n\
                \t.cfi_rel_offset x5, 312\n\
                \t.cfi_rel_offset x6, 320\n\
                \t.cfi_rel_offset x7, 328\n\
                \t.cfi_rel_offset x8, 336\n\
                \t.cfi_rel_offset x9, 344\n\
                \t.cfi_rel_offset x10, 352\n\
                \t.cfi_rel_offset x11, 360\n\
                \t.cfi_rel_offset x12, 368\n\
                \t.cfi_rel_offset x13, 376\n\
                \t.cfi_rel_offset x14, 384\n\
                \t.cfi_rel_offset x15, 392\n\
                \t.cfi_rel_offset x16, 400\n\
                \t.cfi_rel_offset x17, 408\n\
                \t.cfi_rel_offset x19, 416\n\
                \t.cfi_rel_offset x20, 424\n\
                \t.cfi_rel_offset x21, 432\n\
                \t.cfi_rel_offset x22, 440\n\
                \t.cfi_rel_offset x23, 448\n\
                \t.cfi_rel_offset x24, 456\n\
                \t.cfi_rel_offset x25, 464\n\
                \t.cfi_rel_offset x26, 472\n\
                \t.cfi_rel_offset x27, 480\n\
                \t.cfi_rel_offset x28, 488\n\
                \t.cfi_rel_offset x29, 496\n\
                \t.cfi_rel_offset x30, 504\n";
            let save_refs_args_seed = "\t.cfi_def_cfa sp, 224\n\
                \t.cfi_rel_offset x20, 136\n\
                \t.cfi_rel_offset x21, 144\n\
                \t.cfi_rel_offset x22, 152\n\
                \t.cfi_rel_offset x23, 160\n\
                \t.cfi_rel_offset x24, 168\n\
                \t.cfi_rel_offset x25, 176\n\
                \t.cfi_rel_offset x26, 184\n\
                \t.cfi_rel_offset x27, 192\n\
                \t.cfi_rel_offset x28, 200\n\
                \t.cfi_rel_offset x29, 208\n\
                \t.cfi_rel_offset x30, 216\n";
            let generic_jni_seed = "\t.cfi_def_cfa x28, 224\n\
                \t.cfi_rel_offset x20, 136\n\
                \t.cfi_rel_offset x21, 144\n\
                \t.cfi_rel_offset x22, 152\n\
                \t.cfi_rel_offset x23, 160\n\
                \t.cfi_rel_offset x24, 168\n\
                \t.cfi_rel_offset x25, 176\n\
                \t.cfi_rel_offset x26, 184\n\
                \t.cfi_rel_offset x27, 192\n\
                \t.cfi_rel_offset x28, 200\n\
                \t.cfi_rel_offset x29, 208\n\
                \t.cfi_rel_offset x30, 216\n";
            for line in normalized.lines() {
                let trimmed = line.trim();
                if current_entrypoint == "_art_quick_osr_stub" && trimmed == ".cfi_restore_state" {
                    // .Losr_entry starts a second FDE seeded below with the
                    // remembered 192-byte frame state. A restore-state opcode
                    // cannot cross an FDE boundary and would be both invalid
                    // and redundant after that explicit seed.
                    continue;
                }
                if matches!(
                    current_entrypoint.as_str(),
                    "_art_quick_check_instance_of"
                        | "_art_quick_aput_obj"
                        | "_art_quick_test_suspend"
                        | "_art_quick_implicit_suspend"
                        | "_art_quick_proxy_invoke_handler"
                        | "_art_quick_method_entry_hook"
                        | "_art_quick_method_exit_hook"
                        | "_art_quick_generic_jni_trampoline"
                ) && matches!(trimmed, ".cfi_remember_state" | ".cfi_restore_state")
                {
                    // The alternate paths below are separate FDEs with an
                    // explicit initial state. Mach-O cannot carry the DWARF
                    // remember-state stack across those FDE boundaries.
                    continue;
                }
                let split_default_path = matches!(
                    trimmed,
                    ".Lthrow_class_cast_exception_for_bitstring_check:"
                        | ".Laput_obj_check_assignability:"
                        | ".Laput_obj_gc_marking:"
                        | ".Lconflict_trampoline:"
                        | "_art_quick_read_barrier_mark_introspection_gc_roots:"
                        | ".Ldarwin_mark_introspection:"
                        | ".Ldarwin_mark_introspection_unmarked:"
                        | ".Ldarwin_mark_introspection_forwarding_address:"
                );
                let split_imt_exception = current_entrypoint
                    == "_art_quick_imt_conflict_trampoline"
                    && trimmed.starts_with("Ltmp")
                    && trimmed.ends_with(':');
                if split_default_path || split_imt_exception {
                    transformed.push_str("\t.cfi_endproc\n");
                    if trimmed == "_art_quick_read_barrier_mark_introspection_gc_roots:" {
                        transformed.push_str(".Ldarwin_mark_introspection_gc_roots_alias:\n");
                    } else {
                        transformed.push_str(line);
                        transformed.push('\n');
                    }
                    transformed.push_str("\t.cfi_startproc\n");
                    split_control_paths += 1;
                    continue;
                }
                let split_slow_path = trimmed.starts_with(".Lslow_pathart_quick_alloc_")
                    || trimmed.starts_with(".Lslow_rb_art_quick_read_barrier_mark_reg")
                    || trimmed == ".Lhandle_overflow:"
                    || trimmed == ".Lhandle_overflow_exit:";
                if split_slow_path {
                    // These AOSP entrypoints return directly from a frameless
                    // fast path and place a framed slow path later in the same
                    // symbol. ELF permits both CFI regions in one FDE; Mach-O
                    // requires monotonically shaped unwind programs. Two
                    // adjacent FDEs describe the same PCs and frame rules.
                    transformed.push_str("\t.cfi_endproc\n");
                    transformed.push_str(line);
                    transformed.push('\n');
                    transformed.push_str("\t.cfi_startproc\n");
                    split_slow_paths += 1;
                    continue;
                }
                let split_seed = match trimmed {
                    ".Lcall_method_instance:" | ".Lcall_method_static:" => Some(
                        "\t.cfi_def_cfa x29, 64\n\
                         \t.cfi_rel_offset x4, 0\n\
                         \t.cfi_rel_offset x5, 8\n\
                         \t.cfi_rel_offset x19, 24\n\
                         \t.cfi_rel_offset x20, 32\n\
                         \t.cfi_rel_offset x21, 40\n\
                         \t.cfi_rel_offset x29, 48\n\
                         \t.cfi_rel_offset x30, 56\n",
                    ),
                    ".Losr_entry:" => Some(
                        "\t.cfi_def_cfa sp, 192\n\
                         \t.cfi_rel_offset x3, 16\n\
                         \t.cfi_rel_offset x4, 24\n\
                         \t.cfi_rel_offset x19, 32\n\
                         \t.cfi_rel_offset x20, 40\n\
                         \t.cfi_rel_offset x21, 48\n\
                         \t.cfi_rel_offset x22, 56\n\
                         \t.cfi_rel_offset x23, 64\n\
                         \t.cfi_rel_offset x24, 72\n\
                         \t.cfi_rel_offset x25, 80\n\
                         \t.cfi_rel_offset x26, 88\n\
                         \t.cfi_rel_offset x27, 96\n\
                         \t.cfi_rel_offset x28, 104\n\
                         \t.cfi_rel_offset x29, 112\n\
                         \t.cfi_rel_offset x30, 120\n",
                    ),
                    ".Lthrow_class_cast_exception:" => Some(
                        "\t.cfi_def_cfa sp, 32\n\
                         \t.cfi_rel_offset x0, 0\n\
                         \t.cfi_rel_offset x1, 8\n",
                    ),
                    ".Laput_obj_throw_array_store_exception:" => Some(
                        "\t.cfi_def_cfa sp, 32\n\
                         \t.cfi_rel_offset x0, 0\n\
                         \t.cfi_rel_offset x1, 8\n\
                         \t.cfi_rel_offset x2, 16\n\
                         \t.cfi_rel_offset x30, 24\n",
                    ),
                    ".Ltest_suspend_deoptimize:"
                    | ".Limplicit_suspend_deopt:"
                    | ".Lentryhook_deopt:"
                    | ".Lexithook_deopt_or_exception:" => Some(save_everything_seed),
                    ".Lexception_in_proxy:" => Some(save_refs_args_seed),
                    ".Lcall_method_exit_hook_done:"
                    | ".Lcall_method_exit_hook:"
                    | ".Lexception_in_native:" => Some(generic_jni_seed),
                    _ => None,
                };
                if let Some(seed) = split_seed {
                    transformed.push_str("\t.cfi_endproc\n");
                    transformed.push_str(line);
                    transformed.push('\n');
                    transformed.push_str("\t.cfi_startproc\n");
                    transformed.push_str(seed);
                    if matches!(
                        trimmed,
                        ".Lthrow_class_cast_exception:"
                            | ".Laput_obj_throw_array_store_exception:"
                            | ".Ltest_suspend_deoptimize:"
                            | ".Limplicit_suspend_deopt:"
                            | ".Lentryhook_deopt:"
                            | ".Lexithook_deopt_or_exception:"
                            | ".Lexception_in_proxy:"
                            | ".Lcall_method_exit_hook_done:"
                            | ".Lcall_method_exit_hook:"
                            | ".Lexception_in_native:"
                    ) {
                        split_control_paths += 1;
                    }
                    continue;
                }
                if let Some(label) = trimmed.strip_suffix(':') {
                    if label.starts_with("_art_quick_") {
                        current_entrypoint.clear();
                        current_entrypoint.push_str(label);
                    }
                }
                if trimmed == ".cfi_startproc" {
                    retained_cfi_start += 1;
                }
                if trimmed == "_art_quick_read_barrier_mark_introspection_arrays:" {
                    transformed.push_str(".Ldarwin_mark_introspection_arrays_alias:\n");
                } else if trimmed == "_art_quick_read_barrier_mark_introspection_gc_roots:" {
                    transformed.push_str(".Ldarwin_mark_introspection_gc_roots_alias:\n");
                } else {
                    transformed.push_str(line);
                    transformed.push('\n');
                }
            }
            if retained_cfi_start == 0 {
                return Err("quick entrypoint CFI normalization retained no functions".into());
            }
            if split_slow_paths != 46 {
                return Err(format!(
                    "quick Mach-O slow-path FDE inventory drift: {split_slow_paths}/46"
                )
                .into());
            }
            if split_control_paths != 19 {
                return Err(format!(
                    "quick Mach-O alternate-path FDE inventory drift: {split_control_paths}/19"
                )
                .into());
            }
            transformed.push_str(
                ".set _art_quick_read_barrier_mark_introspection_arrays, \
                 .Ldarwin_mark_introspection_arrays_alias\n\
                 .set _art_quick_read_barrier_mark_introspection_gc_roots, \
                 .Ldarwin_mark_introspection_gc_roots_alias\n",
            );
            compile_variant("mixed-cfi", &transformed)?;
            let mixed_object = object_dir.join(format!("{assembly}.mixed-cfi.o"));
            let unwind = command_output(
                Command::new("xcrun")
                    .args(["llvm-objdump", "--unwind-info"])
                    .arg(&mixed_object),
            )?;
            let unwind_entries = unwind.matches("Entry at offset").count();
            if unwind_entries < 300
                || !unwind.contains("_art_quick_throw_null_pointer_exception")
                || !unwind.contains("_art_quick_deoptimize_from_compiled_code")
                || !unwind.contains("_art_quick_invoke_custom")
                || !unwind.contains("_art_quick_generic_jni_trampoline")
                || !unwind.contains("_art_quick_read_barrier_mark_introspection")
                || !unwind.contains("_art_quick_method_exit_hook")
            {
                return Err(format!(
                    "quick Mach-O unwind coverage regressed: entries={unwind_entries}"
                )
                .into());
            }
            run_command(
                Command::new("xcrun")
                    .args(["llvm-dwarfdump", "--verify"])
                    .arg(&mixed_object),
            )?;
            continue;
        }

        let isolated_entrypoint = match assembly {
            "jni_entrypoints_arm64.S" => Some("art_jni_dlsym_lookup_critical_stub"),
            "native_entrypoints_arm64.S" => Some("art_jni_dlsym_lookup_stub"),
            _ => None,
        };
        if let Some(entrypoint) = isolated_entrypoint {
            let entry = format!("ENTRY {entrypoint}");
            let end = format!("END {entrypoint}");
            let mut retained = String::with_capacity(preprocessed.len());
            let mut isolated = String::with_capacity(preprocessed.len() / 2);
            let mut isolated_cfi = String::with_capacity(preprocessed.len() / 2);
            let mut in_macro = false;
            let mut in_target = false;
            let mut target_finished = false;
            let mut skipping_other_entrypoint = false;
            let mut found_target = false;
            for line in preprocessed.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with(".macro ") {
                    in_macro = true;
                }
                if !in_macro && trimmed == entry {
                    in_target = true;
                    found_target = true;
                } else if !in_macro && !in_target && trimmed.starts_with("ENTRY ") {
                    skipping_other_entrypoint = true;
                }

                if !in_target {
                    retained.push_str(line);
                    retained.push('\n');
                }
                if !target_finished && !skipping_other_entrypoint && !trimmed.starts_with(".cfi_") {
                    isolated.push_str(line);
                    isolated.push('\n');
                }
                if !target_finished && !skipping_other_entrypoint {
                    isolated_cfi.push_str(line);
                    isolated_cfi.push('\n');
                }

                if !in_macro && in_target && trimmed == end {
                    in_target = false;
                    target_finished = true;
                }
                if !in_macro && skipping_other_entrypoint && trimmed.starts_with("END ") {
                    skipping_other_entrypoint = false;
                }
                if trimmed == ".endm" {
                    in_macro = false;
                }
            }
            if !found_target || retained.contains(&entry) || !isolated.contains(&entry) {
                return Err(format!("failed to isolate {entrypoint} from {assembly}").into());
            }
            let isolated_macro_source =
                generated_dir.join(format!("{assembly}.darwin-isolated-cfi.S"));
            let isolated_expanded_source =
                generated_dir.join(format!("{assembly}.darwin-isolated-expanded-cfi.S"));
            let isolated_cfi = restore_apple_dwarf_expression_macros(&isolated_cfi)?;
            fs::write(&isolated_macro_source, isolated_cfi)?;
            run_command(
                Command::new("clang")
                    .args([
                        "-cc1as",
                        "-triple",
                        "arm64-apple-macosx",
                        "-filetype",
                        "asm",
                        "-o",
                    ])
                    .arg(&isolated_expanded_source)
                    .arg(&isolated_macro_source),
            )?;
            let isolated_expanded = fs::read_to_string(&isolated_expanded_source)?;
            let lowered = lower_dlsym_cfi(assembly, &isolated_expanded)?;
            compile_variant("cfi", &retained)?;
            compile_variant("isolated-cfi", &lowered)?;

            let cfi_object = object_dir.join(format!("{assembly}.isolated-cfi.o"));
            let baseline_source =
                generated_dir.join(format!("{assembly}.darwin-isolated-no-cfi-baseline.S"));
            let baseline_object = object_dir.join(format!("{assembly}.isolated-no-cfi-baseline.o"));
            fs::write(&baseline_source, &isolated)?;
            run_command(
                Command::new("clang")
                    .args(["-arch", "arm64", "-x", "assembler", "-c"])
                    .arg(&baseline_source)
                    .arg("-o")
                    .arg(&baseline_object),
            )?;
            let cfi_text = command_output(Command::new("otool").arg("-t").arg(&cfi_object))?;
            let baseline_text =
                command_output(Command::new("otool").arg("-t").arg(&baseline_object))?;
            let cfi_text = cfi_text.split_once('\n').map_or("", |(_, text)| text);
            let baseline_text = baseline_text.split_once('\n').map_or("", |(_, text)| text);
            if cfi_text != baseline_text {
                return Err(
                    format!("{entrypoint} Mach-O CFI lowering changed executable bytes").into(),
                );
            }
            let unwind = command_output(
                Command::new("xcrun")
                    .args(["llvm-objdump", "--unwind-info"])
                    .arg(&cfi_object),
            )?;
            let expected_unwind_entries = if assembly == "jni_entrypoints_arm64.S" {
                7
            } else {
                2
            };
            let unwind_entries = unwind.matches("Entry at offset").count();
            let symbols = command_output(Command::new("nm").arg(&cfi_object))?;
            if unwind_entries != expected_unwind_entries
                || !symbols.contains(&format!("_{entrypoint}"))
            {
                return Err(format!(
                    "Mach-O unwind coverage for {entrypoint} regressed: entries={unwind_entries}/{expected_unwind_entries}"
                )
                .into());
            }
            run_command(
                Command::new("xcrun")
                    .args(["llvm-dwarfdump", "--verify"])
                    .arg(&cfi_object),
            )?;
        } else {
            let preserve_cfi = assembly == "memcmp16_arm64.S";
            let mut stripped_cfi = 0usize;
            let mut darwin_assembly = String::with_capacity(preprocessed.len());
            for line in preprocessed.lines() {
                if !preserve_cfi && line.trim_start().starts_with(".cfi_") {
                    stripped_cfi += 1;
                } else {
                    darwin_assembly.push_str(line);
                    darwin_assembly.push('\n');
                }
            }
            if !preserve_cfi && stripped_cfi == 0 {
                return Err(format!("expected CFI directives in {assembly}").into());
            }
            if preserve_cfi && !darwin_assembly.contains(".cfi_startproc") {
                return Err(format!("expected retained CFI directives in {assembly}").into());
            }
            compile_variant(
                if preserve_cfi { "cfi" } else { "no-cfi" },
                &darwin_assembly,
            )?;
        }
    }

    let archive = build_dir.join("libart-arm64-darwin.a");
    // Compile and link the ARM64ng Nterp object into the runtime archive.  The
    // runtime eligibility gate remains untouched; this only makes the native
    // implementation available to the existing ART entrypoint selection.
    let nterp_object = build_nterp_arm64ng(root)?;
    objects.push(nterp_object);
    create_archive(&archive, &objects)?;
    println!(
        "build-runtime-arm64: generated ABI constants, Mach-O objects={} archive={}",
        objects.len(),
        archive.display()
    );
    Ok(())
}

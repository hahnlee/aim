use super::*;

/// Generate the upstream ARM64ng Nterp source for Darwin. The object is linked
/// into the production runtime archive and ART's normal eligibility checks
/// decide whether a method receives the Nterp entry point.
pub(crate) fn build_nterp_arm64ng(root: &Path) -> Result<PathBuf> {
    let runtime = root.join("_aosp/art/runtime");
    let templates = runtime.join("interpreter/mterp/arm64ng");
    let generator = runtime.join("interpreter/mterp/gen_mterp.py");
    if !generator.is_file() || !templates.join("main.S").is_file() {
        return Err("ARM64ng Nterp sources are missing; run `art-bootstrap sync` first".into());
    }
    let build_dir = root.join("_build/nterp-arm64ng");
    let staged_templates = build_dir.join("staged/runtime/interpreter/mterp/arm64ng");
    let staged_arch = build_dir.join("staged/runtime/arch/arm64");
    let generated_dir = build_dir.join("generated");
    fs::create_dir_all(&staged_templates)?;
    fs::create_dir_all(&staged_arch)?;
    fs::create_dir_all(&generated_dir)?;
    for name in [
        "arithmetic.S",
        "array.S",
        "control_flow.S",
        "floating_point.S",
        "invoke.S",
        "main.S",
        "object.S",
        "other.S",
    ] {
        let mut source = fs::read_to_string(templates.join(name))?;
        if name == "array.S" {
            darwinize_array_template(&mut source)?;
        } else if name == "control_flow.S" {
            darwinize_control_flow_template(&mut source)?;
        } else if name == "main.S" {
            darwinize_main_template(&mut source)?;
        } else if name == "invoke.S" {
            darwinize_invoke_template(&mut source)?;
        } else if name == "object.S" {
            darwinize_object_template(&mut source)?;
        } else if name == "other.S" {
            darwinize_other_template(&mut source)?;
        }
        audit_darwin_reference_boundaries(name, &source)?;
        fs::write(staged_templates.join(name), source)?;
    }
    let mut asm_support = fs::read_to_string(runtime.join("arch/arm64/asm_support_arm64.S"))?;
    darwinize_asm_support(&mut asm_support)?;
    fs::write(staged_arch.join("asm_support_arm64.S"), asm_support)?;

    let generated = generated_dir.join("mterp_arm64ng_darwin.S");
    run_command(
        Command::new("python3")
            .arg(generator)
            .arg(&generated)
            .args([
                staged_templates.join("arithmetic.S"),
                staged_templates.join("array.S"),
                staged_templates.join("control_flow.S"),
                staged_templates.join("floating_point.S"),
                staged_templates.join("invoke.S"),
                staged_templates.join("main.S"),
                staged_templates.join("object.S"),
                staged_templates.join("other.S"),
            ]),
    )?;
    let mut source = fs::read_to_string(&generated)?;
    // mterp's ARM64ng templates are ELF-oriented and use the Android symbol
    // spelling for direct calls.  Darwin's assembler accepts those names
    // literally (it does not add the Mach-O C underscore), while every C/C++
    // provider in the arm64 archive is emitted with the leading underscore.
    // Keep the upstream call targets and ABI intact, translating only the
    // object-file decoration at this boundary.  Calls emitted through
    // CALL_SYMBOL/LOAD_PC_REL_ADDRESS were already Darwinized in
    // darwinize_asm_support(); these are the remaining direct calls.
    darwinize_direct_call_symbols(&mut source)?;
    let darwin_generated = generated_dir.join("mterp_arm64ng_darwin.symbols.S");
    fs::write(&darwin_generated, &source)?;
    let preprocessed = command_output(
        Command::new("clang")
            .args([
                "-target",
                "arm64-apple-macosx",
                "-E",
                "-x",
                "assembler-with-cpp",
            ])
            .arg(format!(
                "-I{}",
                root.join("_build/runtime-arm64/generated").display()
            ))
            .arg(format!("-I{}", build_dir.join("staged").display()))
            .arg(format!("-I{}", build_dir.join("staged/runtime").display()))
            .arg(format!("-I{}", staged_arch.display()))
            .arg(format!("-I{}", runtime.display()))
            .arg(format!("-I{}", runtime.join("arch/arm64").display()))
            .arg(&darwin_generated),
    )?;
    audit_nterp_source(&source, &preprocessed)?;
    let lowered = lower_nterp_cfi(&preprocessed)?;
    let lowered_source = generated_dir.join("mterp_arm64ng_darwin.lowered.S");
    fs::write(&lowered_source, &lowered)?;
    let object = build_dir.join("mterp_arm64ng_darwin.o");
    run_command(
        Command::new("clang")
            .args(["-target", "arm64-apple-macosx", "-x", "assembler", "-c"])
            .arg(&lowered_source)
            .arg("-o")
            .arg(&object),
    )?;
    let kind = command_output(Command::new("file").arg(&object))?;
    if !kind.contains("Mach-O 64-bit object arm64") {
        return Err(format!("unexpected ARM64ng Nterp object format: {kind}").into());
    }
    audit_nterp_object(&object)?;
    println!(
        "build-nterp-arm64ng: compiled ARM64ng Mach-O handlers=256 object={}",
        object.display()
    );
    Ok(object)
}

fn darwinize_direct_call_symbols(source: &mut String) -> Result<()> {
    let mut rewritten = 0usize;
    let mut lines = Vec::new();
    for line in source.lines() {
        let mut line = line.to_owned();
        let mut changed = false;
        for symbol in [
            "artInstanceOfFromCode",
            "art_quick_",
            "fmodf",
            "fmod",
            "free",
        ] {
            let needle = format!(" {symbol}");
            let replacement = format!(" _{symbol}");
            if line.contains(&needle) {
                line = line.replace(&needle, &replacement);
                changed = true;
            }
        }
        if changed {
            rewritten += 1;
        }
        lines.push(line);
    }
    *source = lines.join("\n");
    source.push('\n');
    if rewritten < 10 {
        return Err(format!(
            "Darwin Nterp direct-call symbol inventory drift: rewrote {rewritten}, expected at least 10"
        )
        .into());
    }
    Ok(())
}

/// Lower the parts of the upstream Nterp CFI program that cannot be represented
/// by Apple's assembler.  The opcode table is emitted with `.org`-pinned
/// 128-byte handlers, so a single ELF FDE would require backwards
/// `advance_loc` deltas at the four return handlers.  Each of those handlers,
/// and the late `nterp_helper` region, gets an adjacent Mach-O FDE instead.
///
/// The assembler also rejects the helper's `.cfi_restore x22`: the generated
/// helper macro can be instantiated without a preceding x22 save in the new
/// FDE.  State-stack operations and macro CFI are therefore lowered to no-ops;
/// explicit CFA/register seeds at FDE boundaries retain the frame contract.
/// This changes unwind metadata only; instruction and table bytes are kept
/// unchanged.
fn lower_nterp_cfi(source: &str) -> Result<String> {
    let mut lowered = String::with_capacity(source.len() + 512);
    let mut remember = 0usize;
    let mut restore = 0usize;
    let mut cfa_splits = 0usize;
    let mut handler_splits = 0usize;
    let mut helper_splits = 0usize;
    let mut in_macro = false;
    let mut real_fde_starts = 0usize;
    let mut dynamic_cfi_seeded = false;

    const NTERP_FRAME_SEED: &str = "    .cfi_escape 0x0f, 7, 0x92, 25, 0x78, 0x06, 0x23, 0xa0, 0x01\n\
    .cfi_rel_offset x19, 64\n\
    .cfi_rel_offset x20, 72\n\
    .cfi_rel_offset x21, 80\n\
    .cfi_rel_offset x22, 88\n\
    .cfi_rel_offset x23, 96\n\
    .cfi_rel_offset x24, 104\n\
    .cfi_rel_offset x25, 112\n\
    .cfi_rel_offset x26, 120\n\
    .cfi_rel_offset x27, 128\n\
    .cfi_rel_offset x28, 136\n\
    .cfi_rel_offset x29, 144\n\
    .cfi_rel_offset x30, 152\n";

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(".macro ") {
            in_macro = true;
        }

        // Split before each return handler.  The split is deliberately keyed
        // to the local labels (rather than NAME_START) because NAME_START is
        // itself an assembler macro and must remain byte-for-byte unchanged.
        if !in_macro
            && matches!(
                trimmed,
                ".L_op_return_void:"
                    | ".L_op_return:"
                    | ".L_op_return_wide:"
                    | ".L_op_return_object:"
            )
        {
            lowered.push_str("    .cfi_endproc\n");
            lowered.push_str(line);
            lowered.push('\n');
            lowered.push_str("    .cfi_startproc\n");
            lowered.push_str(NTERP_FRAME_SEED);
            handler_splits += 1;
            continue;
        }

        // The source keeps this helper physically between ExecuteNterpImpl and
        // its end marker.  Give its transition-frame CFI an actual FDE so the
        // helper's late CFA update never lands outside an active frame.
        if !in_macro && trimmed == "NAME_START nterp_helper" {
            lowered.push_str("    .cfi_endproc\n");
            lowered.push_str(line);
            lowered.push('\n');
            lowered.push_str("    .cfi_startproc\n");
            lowered.push_str(NTERP_FRAME_SEED);
            helper_splits += 1;
            continue;
        }

        if trimmed == ".cfi_remember_state" {
            remember += 1;
            continue;
        }
        if trimmed == ".cfi_restore_state" {
            restore += 1;
            continue;
        }
        if trimmed == ".cfi_def_cfa sp, (12 * 8 + 8 * 8)" {
            cfa_splits += 1;
            continue;
        }
        // The generated interpreter table uses `.org` to pin every 128-byte
        // handler.  Apple's assembler cannot encode register-state updates
        // across those non-linear CFI locations (it reports invalid
        // advance_loc).  Keep explicit FDE boundaries and seeds above, but
        // lower state operations and macro-emitted restores to no-ops.  This
        // is metadata-only and leaves every instruction and table offset
        // unchanged.
        if trimmed.starts_with(".cfi_")
            && trimmed != ".cfi_startproc"
            && trimmed != ".cfi_endproc"
            && !trimmed.starts_with(".cfi_escape")
        {
            continue;
        }
        if trimmed.starts_with(".cfi_escape") {
            if dynamic_cfi_seeded {
                continue;
            }
            dynamic_cfi_seeded = true;
        }
        lowered.push_str(line);
        lowered.push('\n');
        if trimmed == ".cfi_startproc" {
            if in_macro {
                // ENTRY/END are assembler macros.  Seeding here means every
                // expansion starts with a valid CFA, without leaking a CFI
                // directive into the surrounding source FDE.
                lowered.push_str("    .cfi_def_cfa sp, 0\n");
                dynamic_cfi_seeded = false;
            } else {
                real_fde_starts += 1;
                if real_fde_starts == 2 {
                    lowered.push_str(NTERP_FRAME_SEED);
                    dynamic_cfi_seeded = true;
                } else {
                    lowered.push_str("    .cfi_def_cfa sp, 0\n");
                    dynamic_cfi_seeded = false;
                }
            }
        }
        if trimmed == ".endm" {
            in_macro = false;
        }
    }
    if remember != 5 || restore != 5 || cfa_splits != 5 {
        return Err(format!(
            "ARM64ng Nterp CFI inventory drift: remember={remember}/5 restore={restore}/5 cfa_splits={cfa_splits}/5"
        )
        .into());
    }
    if handler_splits != 4 || helper_splits != 1 {
        return Err(format!(
            "ARM64ng Nterp FDE split inventory drift: handlers={handler_splits}/4 helper={helper_splits}/1"
        )
        .into());
    }
    // Mach-O requires page-relative relocations for external addresses and
    // assembler-local targets for conditional branches.  Keep the public
    // symbols available to the linker while routing the instruction-local
    // references through local labels.
    let lowered = lowered
        // The table is in this object, so use its local label and avoid an
        // ADR relocation against a private-external symbol.
        .replace("adr x24, artNterpAsmInstructionStart", "adr x24, .L_op_nop")
        // Give every ENTRY expansion a local alias.  This keeps conditional
        // branches local while retaining the public underscore symbol.
        .replace(
            "    .balign \\alignment\n_\\name:\n    .cfi_startproc",
            "    .balign \\alignment\n    .set \\name, .L_\\name\n    .set _\\name, .L_\\name\n.L_\\name:\n    .cfi_startproc",
        )
        .replace("bl \\helper", "bl _\\helper")
        .replace("bl Nterp", "bl _Nterp")
        .replace("_ZN3art7Runtime9instance_E", "__ZN3art7Runtime9instance_E@PAGE")
        .replace(
            "#:lo12:__ZN3art7Runtime9instance_E@PAGE",
            "__ZN3art7Runtime9instance_E@PAGEOFF",
        );
    Ok(lowered)
}

fn audit_nterp_object(object: &Path) -> Result<()> {
    let symbols = command_output(Command::new("nm").arg("-nm").arg(object))?;
    let undefined = command_output(Command::new("nm").arg("-u").arg(object))?;
    if undefined.lines().any(|line| {
        let symbol = line.split_whitespace().last().unwrap_or_default();
        symbol.starts_with("art_quick_")
            || symbol == "artInstanceOfFromCode"
            || matches!(symbol, "fmod" | "fmodf" | "free")
    }) {
        return Err("ARM64ng Nterp object retains an ELF-spelled direct call symbol".into());
    }
    for symbol in [
        "_ExecuteNterpImpl",
        "_ExecuteNterpWithClinitImpl",
        "_EndExecuteNterpImpl",
        "_EndExecuteNterpWithClinitImpl",
        "_artNterpAsmInstructionStart",
        "_artNterpAsmInstructionEnd",
    ] {
        if !symbols.lines().any(|line| line.ends_with(symbol)) {
            return Err(format!("ARM64ng Nterp symbol missing: {symbol}").into());
        }
    }
    let start = symbol_value(&symbols, "_artNterpAsmInstructionStart")?;
    let end = symbol_value(&symbols, "_artNterpAsmInstructionEnd")?;
    if end.saturating_sub(start) != 32_768 {
        return Err(format!(
            "ARM64ng Nterp instruction table size drift: start=0x{start:x} end=0x{end:x} delta=0x{:x}",
            end.saturating_sub(start)
        )
        .into());
    }
    let sections = command_output(Command::new("otool").arg("-l").arg(object))?;
    if !sections.contains("__eh_frame") {
        return Err("ARM64ng Nterp object has no __eh_frame section".into());
    }
    run_command(
        Command::new("xcrun")
            .args(["llvm-dwarfdump", "--verify"])
            .arg(object),
    )?;
    Ok(())
}

fn symbol_value(nm_output: &str, symbol: &str) -> Result<u64> {
    let line = nm_output
        .lines()
        .find(|line| line.ends_with(symbol))
        .ok_or_else(|| format!("ARM64ng Nterp symbol missing: {symbol}"))?;
    let value = line
        .split_whitespace()
        .find_map(|word| u64::from_str_radix(word, 16).ok())
        .ok_or_else(|| format!("ARM64ng Nterp symbol has no address: {symbol}"))?;
    Ok(value)
}

fn darwinize_asm_support(source: &mut String) -> Result<()> {
    replace_required(
        source,
        ".macro LOAD_PC_REL_ADDRESS reg, symbol\n    adr \\reg, \\symbol\n.endm\n",
        ".macro LOAD_PC_REL_ADDRESS reg, symbol\n    adr \\reg, _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        ".macro CALL_SYMBOL symbol\n    bl \\symbol\n.endm\n",
        ".macro CALL_SYMBOL symbol\n    bl _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        ".macro BRANCH_SYMBOL symbol\n    b \\symbol\n.endm\n",
        ".macro BRANCH_SYMBOL symbol\n    b _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        ".macro BRANCH_SYMBOL_CBZ reg, symbol\n    cbz \\reg, \\symbol\n.endm\n",
        ".macro BRANCH_SYMBOL_CBZ reg, symbol\n    cbz \\reg, _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        ".macro BRANCH_SYMBOL_NE symbol\n    b.ne \\symbol\n.endm\n",
        ".macro BRANCH_SYMBOL_NE symbol\n    b.ne _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        ".macro BRANCH_SYMBOL_EQ symbol\n    b.eq \\symbol\n.endm\n",
        ".macro BRANCH_SYMBOL_EQ symbol\n    b.eq _\\symbol\n.endm\n",
    )?;
    replace_required(
        source,
        "    .type \\name, #function\n    .hidden \\name  // Hide this as a global symbol, so we do not incur plt calls.\n    .global \\name\n    .balign \\alignment\n\\name:\n",
        "    .private_extern _\\name\n    .global _\\name\n    .balign \\alignment\n_\\name:\n",
    )?;
    replace_required(source, "    .size \\name, .-\\name\n", "")?;
    replace_required(
        source,
        "    adrp \\reg, _ZN3art7Runtime9instance_E\n\n    ldr \\reg, [\\reg, #:lo12:_ZN3art7Runtime9instance_E]\n",
        "    adrp \\reg, __ZN3art7Runtime9instance_E@PAGE\n\n    ldr \\reg, [\\reg, __ZN3art7Runtime9instance_E@PAGEOFF]\n",
    )
    .or_else(|_| Ok::<(), Box<dyn std::error::Error>>(()))?;
    Ok(())
}

fn darwinize_main_template(source: &mut String) -> Result<()> {
    darwinize_reference_macros(source)?;
    darwinize_write_barrier(source)?;
    // cfi_asm_support.h makes this macro a no-op on Darwin, but Nterp's CFA
    // is genuinely dynamic: *(xREFS - 8) + CALLEE_SAVES_SIZE. Reify the AOSP
    // DW_CFA_def_cfa_expression before preprocessing so stack walkers retain
    // the real frame contract on Apple's assembler.
    const DYNAMIC_NTERP_CFA: &str = ".cfi_escape 0x0f, 7, 0x92, 25, 0x78, 0x06, 0x23, 0xa0, 0x01";
    *source = source.replace(
        "CFI_DEF_CFA_BREG_PLUS_UCONST \\cfi_refs, -8, CALLEE_SAVES_SIZE",
        DYNAMIC_NTERP_CFA,
    );
    *source = source.replace(
        "CFI_DEF_CFA_BREG_PLUS_UCONST CFI_REFS, -8, CALLEE_SAVES_SIZE",
        DYNAMIC_NTERP_CFA,
    );
    replace_required(
        source,
        "OAT_ENTRY ExecuteNterpWithClinitImpl\n    .cfi_startproc\n",
        "OAT_ENTRY ExecuteNterpWithClinitImpl\n    .cfi_startproc\n    DARWIN_NORMALIZE_ART_METHOD\n",
    )?;
    replace_required(
        source,
        "OAT_ENTRY ExecuteNterpImpl\n    .cfi_startproc\n",
        "OAT_ENTRY ExecuteNterpImpl\n    .cfi_startproc\n    DARWIN_NORMALIZE_ART_METHOD\n",
    )?;
    replace_required(
        source,
        "    .type \\name, #function\n    .hidden \\name\n    .global \\name\n    .balign 16\n\\name:\n",
        "    .private_extern _\\name\n    .global _\\name\n    .balign 16\n_\\name:\n",
    )?;
    replace_required(source, "    .size \\name, .-\\name\n", "")?;
    // Keep the source's plain intra-object branches local while publishing
    // the Darwin underscore-prefixed ABI symbol.  Local aliases avoid ADR/
    // conditional-branch relocations that Apple's assembler cannot encode.
    *source = source.replace(
        "    .balign 16\n_\\name:\n",
        "    .balign 16\n    .set \\name, .L_\\name\n    .set _\\name, .L_\\name\n.L_\\name:\n",
    );
    replace_required(
        source,
        "w10, w11 .Lgpr_setup_finished_range_\\suffix",
        "w10, w11, .Lgpr_setup_finished_range_\\suffix",
    )?;
    replace_required(
        source,
        "    .type \\name, #function\n    .hidden \\name  // Hide this as a global symbol, so we do not incur plt calls.\n    .global \\name\n    /* Cache alignment for function entry */\n    .balign 16\n\\name:\n",
        "    .set \\name, .L_\\name\n    .balign 16\n.L_\\name:\n",
    )?;
    for (old, new) in [
        (
            "    .type EndExecuteNterpWithClinitImpl, #function\n    .hidden EndExecuteNterpWithClinitImpl\n    .global EndExecuteNterpWithClinitImpl\nEndExecuteNterpWithClinitImpl:",
            "    .private_extern _EndExecuteNterpWithClinitImpl\n    .global _EndExecuteNterpWithClinitImpl\n_EndExecuteNterpWithClinitImpl:",
        ),
        (
            "    .type EndExecuteNterpImpl, #function\n    .hidden EndExecuteNterpImpl\n    .global EndExecuteNterpImpl\nEndExecuteNterpImpl:",
            "    .private_extern _EndExecuteNterpImpl\n    .global _EndExecuteNterpImpl\n_EndExecuteNterpImpl:",
        ),
        (
            "    .type artNterpAsmInstructionEnd, #function\n    .hidden artNterpAsmInstructionEnd\n    .global artNterpAsmInstructionEnd\nartNterpAsmInstructionEnd:",
            "    .private_extern _artNterpAsmInstructionEnd\n    .global _artNterpAsmInstructionEnd\n    .set artNterpAsmInstructionEnd, .L_artNterpAsmInstructionEnd\n    .set _artNterpAsmInstructionEnd, .L_artNterpAsmInstructionEnd\n.L_artNterpAsmInstructionEnd:",
        ),
        (
            "    .type artNterpAsmInstructionStart, #function\n    .hidden artNterpAsmInstructionStart\n    .global artNterpAsmInstructionStart\nartNterpAsmInstructionStart = .L_op_nop",
            "    .private_extern _artNterpAsmInstructionStart\n    .global _artNterpAsmInstructionStart\n_artNterpAsmInstructionStart = .L_op_nop",
        ),
    ] {
        replace_required(source, old, new)?;
    }
    Ok(())
}

fn darwinize_reference_macros(source: &mut String) -> Result<()> {
    // Nterp's managed ABI deliberately keeps references in 32-bit W
    // registers. Native object dereferences and C++ quick entrypoints need the
    // Darwin heap window restored in the corresponding X register. Keep both
    // operations explicit so call sites document whether null is possible.
    replace_required(
        source,
        ".macro GET_VREG reg, vreg\n    ldr     \\reg, [xFP, \\vreg, uxtw #2]\n.endm\n",
        ".macro GET_VREG reg, vreg\n    ldr     \\reg, [xFP, \\vreg, uxtw #2]\n.endm\n\
.macro DARWIN_DECODE_NON_NULL_HEAP_REF xreg\n\
    orr     \\xreg, \\xreg, #AIM_REFERENCE_BASE\n\
.endm\n\
.macro DARWIN_NORMALIZE_ART_METHOD\n\
    cbz     x0, 988f\n\
    mov     ip2, x0\n\
    lsr     ip2, ip2, #32\n\
    cbnz    ip2, 988f\n\
    mov     ip2, #AIM_REFERENCE_BASE\n\
    orr     x0, x0, ip2\n\
988:\n\
.endm\n\
.macro DARWIN_DECODE_NULLABLE_HEAP_REF xreg, wreg\n\
    cbz     \\wreg, 987f\n\
    DARWIN_DECODE_NON_NULL_HEAP_REF \\xreg\n\
987:\n\
.endm\n",
    )
}

fn replace_required_count(
    source: &mut String,
    from: &str,
    to: &str,
    expected: usize,
) -> Result<()> {
    let actual = source.matches(from).count();
    if actual != expected {
        return Err(format!(
            "locked ART source fragment count drift: found {actual}, expected {expected}: {from}"
        )
        .into());
    }
    *source = source.replace(from, to);
    Ok(())
}

fn darwinize_write_barrier(source: &mut String) -> Result<()> {
    // The thread's biased card-table pointer was constructed for native heap
    // addresses. Rebuild the native address in a 64-bit scratch register
    // before applying CardTable::kCardShift, matching the ARM64 compiler's
    // ReferenceCodegenARM64::DecodeNonNull write-barrier path.
    replace_required(
        source,
        "   ldr     ip, [xSELF, #THREAD_CARD_TABLE_OFFSET]\n   lsr     wip2, \\holder, #CARD_TABLE_CARD_SHIFT\n   strb    wip, [ip, ip2]\n",
        "   ldr     ip, [xSELF, #THREAD_CARD_TABLE_OFFSET]\n   mov     wip2, \\holder\n   DARWIN_DECODE_NON_NULL_HEAP_REF ip2\n   lsr     ip2, ip2, #CARD_TABLE_CARD_SHIFT\n   strb    wip, [ip, ip2]\n",
    )
}

fn darwinize_array_template(source: &mut String) -> Result<()> {
    for (android, darwin) in [
        (
            "    cbz     x0, common_errNullObject    // bail if null array object.\n    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- arrayObj->length\n",
            "    cbz     x0, common_errNullObject    // bail if null array object.\n    DARWIN_DECODE_NON_NULL_HEAP_REF x0\n    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- arrayObj->length\n",
        ),
        (
            "    cbz     w0, common_errNullObject    // bail if null\n    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]     // w3<- arrayObj->length\n",
            // art_quick_aput_obj has its own Darwin compressed-reference
            // lowering, so use a decoded copy for the bounds load and keep
            // x0 compressed only for that object helper.
            "    cbz     w0, common_errNullObject    // bail if null\n    mov     w10, w0\n    DARWIN_DECODE_NON_NULL_HEAP_REF x10\n    ldr     w3, [x10, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- arrayObj->length\n    .if !$is_object\n    mov     x0, x10\n    .endif\n",
        ),
        (
            "    cbz     w0, common_errNullObject    // bail if null\n    FETCH_ADVANCE_INST 1                // advance rPC, load rINST\n    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- array length\n",
            "    cbz     w0, common_errNullObject    // bail if null\n    DARWIN_DECODE_NON_NULL_HEAP_REF x0\n    FETCH_ADVANCE_INST 1                // advance rPC, load rINST\n    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- array length\n",
        ),
        (
            // FillArrayData takes an ObjPtr rather than the compressed
            // managed register representation. Preserve null for its normal
            // runtime NPE path and decode only a non-null array.
            "    GET_VREG w1, w3                     // w1<- vAA (array object)\n    add     x0, xPC, x0, lsl #1         // x0<- PC + ssssssssBBBBbbbb*2 (array data off.)\n    bl      art_quick_handle_fill_data\n",
            "    GET_VREG w1, w3                     // w1<- vAA (array object)\n    DARWIN_DECODE_NULLABLE_HEAP_REF x1, w1\n    add     x0, xPC, x0, lsl #1         // x0<- PC + ssssssssBBBBbbbb*2 (array data off.)\n    bl      art_quick_handle_fill_data\n",
        ),
    ] {
        replace_required(source, android, darwin)?;
    }
    Ok(())
}

fn darwinize_control_flow_template(source: &mut String) -> Result<()> {
    // DeliverException consumes a native Throwable*. Preserve null so the
    // quick entrypoint retains AOSP's throw-null behavior.
    replace_required(
        source,
        "  GET_VREG w0, w2                      // r0<- vAA (exception object)\n  mov x1, xSELF\n  bl art_quick_deliver_exception\n",
        "  GET_VREG w0, w2                      // r0<- vAA (exception object)\n  DARWIN_DECODE_NULLABLE_HEAP_REF x0, w0\n  mov x1, xSELF\n  bl art_quick_deliver_exception\n",
    )
}

fn darwinize_other_template(source: &mut String) -> Result<()> {
    // The monitor quick entrypoints take native Object* arguments. They own
    // the null/exception semantics, so decode without branching around them.
    for helper in ["art_quick_lock_object", "art_quick_unlock_object"] {
        replace_required(
            source,
            &format!("    GET_VREG w0, w2\n    bl {helper}\n"),
            &format!(
                "    GET_VREG w0, w2\n    DARWIN_DECODE_NULLABLE_HEAP_REF x0, w0\n    bl {helper}\n"
            ),
        )?;
    }
    Ok(())
}

fn darwinize_invoke_template(source: &mut String) -> Result<()> {
    // Keep w1 as the compressed managed `this` argument expected by ART's
    // invoke ABI. Decode copies used only for receiver/class dereferences in
    // virtual and interface dispatch.
    for (android, darwin) in [
        (
            "   GET_VREG w1, w1\n   // Note: if w1 is null, this will be handled by our SIGSEGV handler.\n   ldr w0, [x1, #MIRROR_OBJECT_CLASS_OFFSET]\n   UNPOISON_HEAP_REF w0\n   add w0, w0, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
            "   GET_VREG w1, w1\n   cbz w1, common_errNullObject\n   mov w0, w1\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   ldr w0, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n   UNPOISON_HEAP_REF w0\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   add x0, x0, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
        ),
        (
            "   GET_VREG w1, w1\n   // Note: if w1 is null, this will be handled by our SIGSEGV handler.\n   ldr w2, [x1, #MIRROR_OBJECT_CLASS_OFFSET]\n   UNPOISON_HEAP_REF w2\n",
            "   GET_VREG w1, w1\n   cbz w1, common_errNullObject\n   mov w2, w1\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   ldr w2, [x2, #MIRROR_OBJECT_CLASS_OFFSET]\n   UNPOISON_HEAP_REF w2\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n",
        ),
    ] {
        replace_required(source, android, darwin)?;
    }
    // The Object-method arm of invoke-interface dispatch indexes the
    // receiver class vtable.  The class reference was decoded into X2 above;
    // using the W alias here truncates the Darwin heap-window base and turns
    // the subsequent load into a low-address fault.  Keep the native class
    // address in X2 while leaving the managed compressed value in W2 for the
    // other invoke paths.
    replace_required(
        source,
        "   add w2, w2, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
        "   add x2, x2, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
    )?;
    Ok(())
}

fn darwinize_object_template(source: &mut String) -> Result<()> {
    darwinize_object_managed_boundaries(source)?;
    darwinize_object_static_field_template(source)
}

fn darwinize_object_managed_boundaries(source: &mut String) -> Result<()> {
    // Decode only address-bearing X registers. Their W aliases continue to
    // carry the logical compressed value used for comparisons, heap stores,
    // read-barrier conventions, and the managed call ABI.
    for (android, darwin) in [
        (
            "   GET_VREG w0, w2                     // w0<- vA (object)\n   cbz     w0, .L${opcode}_resume\n   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "   GET_VREG w0, w2                     // w0<- vA (object)\n   cbz     w0, .L${opcode}_resume\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
        ),
        (
            "   GET_VREG w0, w2                     // w0<- vB (object)\n   cbz     w0, .L${opcode}_resume\n   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "   GET_VREG w0, w2                     // w0<- vB (object)\n   cbz     w0, .L${opcode}_resume\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
        ),
        (
            "5:\n   // Class in w1 is an array, w3 is the component type.\n",
            "5:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   DARWIN_DECODE_NON_NULL_HEAP_REF x3\n   // Class in w1 is an array, w3 is the component type.\n",
        ),
        (
            "3:\n   // Class in x1 is an array, x3 is the component type of x1, and x2 is the class of the object.\n",
            "3:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   DARWIN_DECODE_NON_NULL_HEAP_REF x3\n   // Class in x1 is an array, x3 is the component type of x1, and x2 is the class of the object.\n",
        ),
        (
            "3:\n   EXPORT_PC\n   bl      art_quick_check_instance_of\n",
            "3:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x1\n   EXPORT_PC\n   bl      art_quick_check_instance_of\n",
        ),
        (
            "5:\n   EXPORT_PC\n   bl      artInstanceOfFromCode\n",
            "5:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x1\n   EXPORT_PC\n   bl      artInstanceOfFromCode\n",
        ),
        (
            "   cbz     w3, common_errNullObject    // object was null\n   .if $wide\n",
            "   cbz     w3, common_errNullObject    // object was null\n   DARWIN_DECODE_NON_NULL_HEAP_REF x3\n   .if $wide\n",
        ),
        (
            "   cbz     w3, common_errNullObject    // object was null\n   add     x3, x3, x0\n",
            "   cbz     w3, common_errNullObject    // object was null\n   DARWIN_DECODE_NON_NULL_HEAP_REF x3\n   add     x3, x3, x0\n",
        ),
        (
            "   cbz w2, common_errNullObject\n   .if $wide\n",
            "   cbz w2, common_errNullObject\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   .if $wide\n",
        ),
        (
            "   cbz     w2, common_errNullObject\n   add     x3, x2, x0\n",
            "   cbz     w2, common_errNullObject\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   add     x3, x2, x0\n",
        ),
    ] {
        replace_required(source, android, darwin)?;
    }
    replace_required_count(
        source,
        "1:\n   ldr     w2, [x2, #MIRROR_CLASS_SUPER_CLASS_OFFSET]\n",
        "1:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   ldr     w2, [x2, #MIRROR_CLASS_SUPER_CLASS_OFFSET]\n",
        2,
    )?;
    replace_required_count(
        source,
        "   cbz     w2, 2b\n",
        "   cbz     w2, 2b\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n",
        2,
    )?;

    Ok(())
}

fn darwinize_object_static_field_template(source: &mut String) -> Result<()> {
    // ART_FIELD_DECLARING_CLASS_OFFSET stores a compressed heap reference.
    // Android can use that 32-bit value as a native address because its heap
    // lives below 4 GiB.  Darwin instead maps the same logical 4 GiB window at
    // kArtCompressedReferenceBase, so restore the high base after the read
    // barrier and before dereferencing the declaring class.  Both the cached
    // and resolver paths reach separate resume labels.
    for (resume, decoded) in [
        (
            ".L${opcode}_resume_after_read_barrier:\n   .if $wide\n   ldr     x0, [x0, x1]\n",
            ".L${opcode}_resume_after_read_barrier:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   .if $wide\n   ldr     x0, [x0, x1]\n",
        ),
        (
            ".L${opcode}_slow_path_resume_after_read_barrier:\n   add     x0, x0, x1\n",
            ".L${opcode}_slow_path_resume_after_read_barrier:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   add     x0, x0, x1\n",
        ),
        (
            ".L${opcode}_resume_after_read_barrier:\n   .if $wide\n   $store  x26, [x0, x1]\n",
            ".L${opcode}_resume_after_read_barrier:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   .if $wide\n   $store  x26, [x0, x1]\n",
        ),
        (
            ".L${opcode}_slow_path_resume_after_read_barrier:\n   add     x1, x0, x1\n",
            ".L${opcode}_slow_path_resume_after_read_barrier:\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   add     x1, x0, x1\n",
        ),
        (
            ".L${opcode}_read_barrier:\n   bl      art_quick_read_barrier_mark_reg00\n   .if $is_object\n   $load   w0, [x0, x1]\n",
            ".L${opcode}_read_barrier:\n   bl      art_quick_read_barrier_mark_reg00\n   .if $is_object\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   $load   w0, [x0, x1]\n",
        ),
    ] {
        replace_required(source, resume, decoded)?;
    }
    Ok(())
}

fn audit_darwin_reference_boundaries(template: &str, source: &str) -> Result<()> {
    // This is the complete ARM64ng template inventory of conversions from a
    // managed compressed reference to either a native dereference or a C++
    // entrypoint argument. Managed invoke arguments, returns, heap stores and
    // read-barrier inputs intentionally remain compressed.
    let non_null = source
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("DARWIN_DECODE_NON_NULL_HEAP_REF ") && !line.contains("\\xreg")
        })
        .count();
    let nullable = source
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with("DARWIN_DECODE_NULLABLE_HEAP_REF ")
        })
        .count();
    let expected = match template {
        "main.S" => (1, 0),         // card-table holder
        "array.S" => (3, 1),        // data/length dereferences + FillArrayData
        "invoke.S" => (4, 0),       // receiver and declaring class temporaries
        "object.S" => (21, 0),      // type checks, field holders, static classes
        "control_flow.S" => (0, 1), // DeliverException
        "other.S" => (0, 2),        // monitor enter/exit
        "arithmetic.S" | "floating_point.S" => (0, 0),
        _ => {
            return Err(
                format!("unknown ARM64ng template in reference inventory: {template}").into(),
            );
        }
    };
    if (non_null, nullable) != expected {
        return Err(format!(
            "Darwin Nterp reference-boundary inventory drift in {template}: non-null={non_null}/{} nullable={nullable}/{}",
            expected.0, expected.1
        )
        .into());
    }
    if template == "main.S" {
        if !source.contains(".macro DARWIN_DECODE_NON_NULL_HEAP_REF xreg")
            || !source.contains(".macro DARWIN_DECODE_NULLABLE_HEAP_REF xreg, wreg")
            || !source.contains(".macro DARWIN_NORMALIZE_ART_METHOD")
        {
            return Err("Darwin Nterp reference decode macros are missing".into());
        }
    } else if source.contains("#0x10000000000") {
        return Err(format!(
            "Darwin Nterp reference lowering in {template} bypasses the shared decode macros"
        )
        .into());
    }
    Ok(())
}

fn audit_nterp_source(source: &str, preprocessed: &str) -> Result<()> {
    if !source.contains("DO NOT EDIT: This file was generated by gen-mterp.py") {
        return Err("Nterp generator did not emit its provenance marker".into());
    }
    let handlers = source
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.starts_with("NAME_START nterp_op_") && !line.ends_with("_slow_path")
        })
        .count();
    if handlers != 256 {
        return Err(format!("ARM64ng Nterp handler inventory drift: {handlers}/256").into());
    }
    if source.matches(".macro DARWIN_NORMALIZE_ART_METHOD").count() != 1 {
        return Err("Darwin Nterp ArtMethod normalization macro inventory drift".into());
    }
    audit_art_method_entry_normalization(
        source,
        "ExecuteNterpWithClinitImpl",
        "_EndExecuteNterpWithClinitImpl:",
        "ldr wip, [x0, #ART_METHOD_DECLARING_CLASS_OFFSET]",
    )?;
    audit_art_method_entry_normalization(
        source,
        "ExecuteNterpImpl",
        "_EndExecuteNterpImpl:",
        "ldr xPC, [x0, #ART_METHOD_DATA_OFFSET_64]",
    )?;
    for symbol in [
        "_\\name:",
        "_EndExecuteNterpImpl:",
        "_EndExecuteNterpWithClinitImpl:",
        "_artNterpAsmInstructionStart",
        "_artNterpAsmInstructionEnd:",
    ] {
        if !source.contains(symbol) && !preprocessed.contains(symbol) {
            return Err(format!("Darwin Nterp symbol missing: {symbol}").into());
        }
    }
    if preprocessed.lines().any(|line| {
        let line = line.trim();
        line.starts_with(".type ") || line.starts_with(".hidden ") || line.starts_with(".size ")
    }) {
        return Err("ELF-only directives escaped ARM64ng Mach-O lowering".into());
    }
    let starts = preprocessed.matches(".cfi_startproc").count();
    let ends = preprocessed.matches(".cfi_endproc").count();
    if starts == 0 || starts != ends {
        return Err(format!("Nterp CFI inventory mismatch: starts={starts} ends={ends}").into());
    }
    Ok(())
}

fn audit_art_method_entry_normalization(
    source: &str,
    entry: &str,
    end_marker: &str,
    first_dereference: &str,
) -> Result<()> {
    let entry_marker = format!("OAT_ENTRY {entry}");
    let start = source
        .find(&entry_marker)
        .ok_or_else(|| format!("Darwin Nterp entry missing: {entry}"))?;
    let region = &source[start..];
    let end = region
        .find(end_marker)
        .ok_or_else(|| format!("Darwin Nterp end marker missing for {entry}: {end_marker}"))?;
    let region = &region[..end];
    let normalization = region
        .find("DARWIN_NORMALIZE_ART_METHOD")
        .ok_or_else(|| format!("Darwin Nterp entry is not normalizing ArtMethod*: {entry}"))?;
    if region[normalization + "DARWIN_NORMALIZE_ART_METHOD".len()..]
        .contains("DARWIN_NORMALIZE_ART_METHOD")
    {
        return Err(
            format!("Darwin Nterp entry normalizes ArtMethod* more than once: {entry}").into(),
        );
    }
    let dereference = region.find(first_dereference).ok_or_else(|| {
        format!("Darwin Nterp first ArtMethod* dereference drifted in {entry}: {first_dereference}")
    })?;
    if normalization > dereference {
        return Err(format!(
            "Darwin Nterp entry dereferences ArtMethod* before normalization: {entry}"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn fixture(handler_count: usize) -> (String, String) {
        let mut source =
            String::from("/* DO NOT EDIT: This file was generated by gen-mterp.py. */\n");
        source.push_str("_\\name:\n");
        for index in 0..handler_count {
            writeln!(source, "NAME_START nterp_op_{index}").unwrap();
        }
        source.push_str(concat!(
            ".macro DARWIN_NORMALIZE_ART_METHOD\n.endm\n",
            "OAT_ENTRY ExecuteNterpWithClinitImpl\n",
            "    DARWIN_NORMALIZE_ART_METHOD\n",
            "    ldr wip, [x0, #ART_METHOD_DECLARING_CLASS_OFFSET]\n",
            "_EndExecuteNterpWithClinitImpl:\n",
            "OAT_ENTRY ExecuteNterpImpl\n",
            "    DARWIN_NORMALIZE_ART_METHOD\n",
            "    ldr xPC, [x0, #ART_METHOD_DATA_OFFSET_64]\n",
            "_EndExecuteNterpImpl:\n",
            "_artNterpAsmInstructionStart\n",
            "_artNterpAsmInstructionEnd:\n",
            ".cfi_startproc\n.cfi_endproc\n",
        ));
        (
            source,
            String::from("_EndExecuteNterpImpl:\n.cfi_startproc\n.cfi_endproc\n"),
        )
    }

    #[test]
    fn nterp_source_audit_is_fail_closed_on_handler_drift() {
        let (source, preprocessed) = fixture(256);
        assert!(audit_nterp_source(&source, &preprocessed).is_ok());
        let (source, preprocessed) = fixture(255);
        assert!(audit_nterp_source(&source, &preprocessed).is_err());
    }

    #[test]
    fn nterp_source_audit_is_fail_closed_on_late_art_method_normalization() {
        let (mut source, preprocessed) = fixture(256);
        source = source.replace(
            "    DARWIN_NORMALIZE_ART_METHOD\n    ldr xPC, [x0, #ART_METHOD_DATA_OFFSET_64]",
            "    ldr xPC, [x0, #ART_METHOD_DATA_OFFSET_64]\n    DARWIN_NORMALIZE_ART_METHOD",
        );
        assert!(audit_nterp_source(&source, &preprocessed).is_err());
    }

    #[test]
    fn direct_call_symbols_get_macho_object_prefix() {
        let mut source = String::from(
            "    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl art_quick_test_suspend\n    bl artInstanceOfFromCode\n    bl fmodf\n    bl free\n",
        );
        darwinize_direct_call_symbols(&mut source).unwrap();
        assert!(source.contains("bl _art_quick_test_suspend"));
        assert!(source.contains("bl _artInstanceOfFromCode"));
        assert!(source.contains("bl _fmodf"));
        assert!(source.contains("bl _free"));
    }

    #[test]
    fn shared_reference_macros_distinguish_nullable_from_non_null() {
        let mut source = String::from(
            ".macro GET_VREG reg, vreg\n    ldr     \\reg, [xFP, \\vreg, uxtw #2]\n.endm\n",
        );
        darwinize_reference_macros(&mut source).unwrap();
        assert!(source.contains(".macro DARWIN_DECODE_NON_NULL_HEAP_REF xreg"));
        assert!(source.contains("orr     \\xreg, \\xreg, #AIM_REFERENCE_BASE"));
        assert!(source.contains(".macro DARWIN_NORMALIZE_ART_METHOD"));
        assert!(source.contains(".macro DARWIN_DECODE_NULLABLE_HEAP_REF xreg, wreg"));
        assert!(source.contains("cbz     \\wreg, 987f"));
    }

    #[test]
    fn throw_and_monitor_quick_boundaries_decode_nullable_objects() {
        let mut control = String::from(
            "  GET_VREG w0, w2                      // r0<- vAA (exception object)\n  mov x1, xSELF\n  bl art_quick_deliver_exception\n",
        );
        darwinize_control_flow_template(&mut control).unwrap();
        assert!(control.contains("DARWIN_DECODE_NULLABLE_HEAP_REF x0, w0"));

        let mut other = String::from(concat!(
            "    GET_VREG w0, w2\n",
            "    bl art_quick_lock_object\n",
            "monitor separator\n",
            "    GET_VREG w0, w2\n",
            "    bl art_quick_unlock_object\n",
        ));
        darwinize_other_template(&mut other).unwrap();
        assert_eq!(
            other
                .matches("DARWIN_DECODE_NULLABLE_HEAP_REF x0, w0")
                .count(),
            2
        );
    }

    #[test]
    fn object_type_and_field_boundaries_decode_only_address_registers() {
        let mut source = String::from(concat!(
            "   GET_VREG w0, w2                     // w0<- vA (object)\n",
            "   cbz     w0, .L${opcode}_resume\n",
            "   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "   GET_VREG w0, w2                     // w0<- vB (object)\n",
            "   cbz     w0, .L${opcode}_resume\n",
            "   ldr     w2, [x0, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "1:\n   ldr     w2, [x2, #MIRROR_CLASS_SUPER_CLASS_OFFSET]\n",
            "1:\n   ldr     w2, [x2, #MIRROR_CLASS_SUPER_CLASS_OFFSET]\n",
            "5:\n   // Class in w1 is an array, w3 is the component type.\n",
            "3:\n   // Class in x1 is an array, x3 is the component type of x1, and x2 is the class of the object.\n",
            "   cbz     w2, 2b\n",
            "   cbz     w2, 2b\n",
            "3:\n   EXPORT_PC\n   bl      art_quick_check_instance_of\n",
            "5:\n   EXPORT_PC\n   bl      artInstanceOfFromCode\n",
            "   cbz     w3, common_errNullObject    // object was null\n",
            "   .if $wide\n",
            "   cbz     w3, common_errNullObject    // object was null\n",
            "   add     x3, x3, x0\n",
            "   cbz w2, common_errNullObject\n",
            "   .if $wide\n",
            "   cbz     w2, common_errNullObject\n",
            "   add     x3, x2, x0\n",
        ));
        darwinize_object_managed_boundaries(&mut source).unwrap();
        assert_eq!(
            source.matches("DARWIN_DECODE_NON_NULL_HEAP_REF ").count(),
            16
        );
        assert!(source.contains("GET_VREG w0, w2                     // w0<- vA (object)\n   cbz"));
        assert!(source.contains(
            "DARWIN_DECODE_NON_NULL_HEAP_REF x1\n   EXPORT_PC\n   bl      art_quick_check_instance_of"
        ));
        assert!(source.contains(
            "cbz     w2, common_errNullObject\n   DARWIN_DECODE_NON_NULL_HEAP_REF x2\n   add     x3, x2, x0"
        ));
    }

    #[test]
    fn reference_boundary_inventory_is_fail_closed() {
        let mut source = "   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n".repeat(21);
        assert!(audit_darwin_reference_boundaries("object.S", &source).is_ok());
        source.pop();
        source.truncate(source.rfind('\n').unwrap_or(0) + 1);
        assert!(audit_darwin_reference_boundaries("object.S", &source).is_err());
    }

    #[test]
    fn object_static_fields_decode_darwin_compressed_declaring_class() {
        let mut source = String::from(
            ".L${opcode}_resume_after_read_barrier:\n   .if $wide\n   ldr     x0, [x0, x1]\n\
             .L${opcode}_slow_path_resume_after_read_barrier:\n   add     x0, x0, x1\n\
             .L${opcode}_resume_after_read_barrier:\n   .if $wide\n   $store  x26, [x0, x1]\n\
             .L${opcode}_slow_path_resume_after_read_barrier:\n   add     x1, x0, x1\n\
             .L${opcode}_read_barrier:\n   bl      art_quick_read_barrier_mark_reg00\n   .if $is_object\n   $load   w0, [x0, x1]\n",
        );
        darwinize_object_static_field_template(&mut source).unwrap();
        assert_eq!(
            source.matches("DARWIN_DECODE_NON_NULL_HEAP_REF x0").count(),
            5
        );
        assert_eq!(
            source
                .matches("resume_after_read_barrier:\n   DARWIN_DECODE")
                .count(),
            4
        );
        assert!(source.contains(
            "bl      art_quick_read_barrier_mark_reg00\n   .if $is_object\n   DARWIN_DECODE_NON_NULL_HEAP_REF x0\n   $load   w0, [x0, x1]"
        ));
    }

    #[test]
    fn write_barrier_indexes_cards_with_decoded_native_holder() {
        let mut source = String::from(
            "   ldr     ip, [xSELF, #THREAD_CARD_TABLE_OFFSET]\n   lsr     wip2, \\holder, #CARD_TABLE_CARD_SHIFT\n   strb    wip, [ip, ip2]\n",
        );
        darwinize_write_barrier(&mut source).unwrap();
        assert!(source.contains("mov     wip2, \\holder"));
        assert!(source.contains("DARWIN_DECODE_NON_NULL_HEAP_REF ip2"));
        assert!(source.contains("lsr     ip2, ip2, #CARD_TABLE_CARD_SHIFT"));
        assert!(!source.contains("lsr     wip2, \\holder"));
    }

    #[test]
    fn invoke_dispatch_decodes_temporaries_but_preserves_this_argument() {
        let mut source = String::from(concat!(
            "   GET_VREG w1, w1\n",
            "   // Note: if w1 is null, this will be handled by our SIGSEGV handler.\n",
            "   ldr w0, [x1, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "   UNPOISON_HEAP_REF w0\n",
            "   add w0, w0, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
            "interface separator\n",
            "   GET_VREG w1, w1\n",
            "   // Note: if w1 is null, this will be handled by our SIGSEGV handler.\n",
            "   ldr w2, [x1, #MIRROR_OBJECT_CLASS_OFFSET]\n",
            "   UNPOISON_HEAP_REF w2\n",
            "   add w2, w2, #MIRROR_CLASS_VTABLE_OFFSET_64\n",
        ));
        darwinize_invoke_template(&mut source).unwrap();
        assert_eq!(
            source.matches("DARWIN_DECODE_NON_NULL_HEAP_REF x0").count(),
            2
        );
        assert_eq!(
            source.matches("DARWIN_DECODE_NON_NULL_HEAP_REF x2").count(),
            2
        );
        assert_eq!(source.matches("GET_VREG w1, w1").count(), 2);
        assert!(!source.contains("ldr w0, [x1"));
        assert!(!source.contains("ldr w2, [x1"));
    }

    #[test]
    fn array_accesses_decode_native_addresses_at_each_boundary() {
        let mut source = String::from(concat!(
            "    cbz     x0, common_errNullObject    // bail if null array object.\n",
            "    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- arrayObj->length\n",
            "aget/aput separator\n",
            "    cbz     w0, common_errNullObject    // bail if null\n",
            "    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]     // w3<- arrayObj->length\n",
            "aput/length separator\n",
            "    cbz     w0, common_errNullObject    // bail if null\n",
            "    FETCH_ADVANCE_INST 1                // advance rPC, load rINST\n",
            "    ldr     w3, [x0, #MIRROR_ARRAY_LENGTH_OFFSET]    // w3<- array length\n",
            "length/fill separator\n",
            "    GET_VREG w1, w3                     // w1<- vAA (array object)\n",
            "    add     x0, xPC, x0, lsl #1         // x0<- PC + ssssssssBBBBbbbb*2 (array data off.)\n",
            "    bl      art_quick_handle_fill_data\n",
        ));
        darwinize_array_template(&mut source).unwrap();
        assert!(source.contains("DARWIN_DECODE_NULLABLE_HEAP_REF x1, w1"));
        assert_eq!(
            source.matches("DARWIN_DECODE_NON_NULL_HEAP_REF x0").count(),
            2
        );
        assert!(source.contains("mov     w10, w0\n    DARWIN_DECODE_NON_NULL_HEAP_REF x10"));
        assert!(source.contains(".if !$is_object\n    mov     x0, x10\n    .endif"));
    }
}

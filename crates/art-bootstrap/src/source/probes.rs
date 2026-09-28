use super::*;

pub(crate) fn probe_asm(root: &Path) -> Result<()> {
    let source = root.join("_aosp/art/runtime/arch/arm64/asm_support_arm64.S");
    if !source.exists() {
        return Err("ARM64 source is missing; run `art-bootstrap sync` first".into());
    }

    let build_dir = root.join("_build/asm-probe");
    let generated_dir = build_dir.join("generated");
    let patched_source_dir = build_dir.join("patched-source");
    fs::create_dir_all(&generated_dir)?;
    fs::create_dir_all(patched_source_dir.join("runtime/arch/arm64"))?;

    let patched_source = patched_source_dir.join("runtime/arch/arm64/asm_support_arm64.S");
    fs::copy(&source, &patched_source)?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0001-arm64-mach-o-assembly.patch"))
            .current_dir(&patched_source_dir),
    )?;

    let patched = fs::read_to_string(&patched_source)?;
    let generated = isolate_asm_support(patched)?;
    let generated_support = generated_dir.join("asm_support_arm64_darwin.S");
    fs::write(&generated_support, generated)?;

    let object = build_dir.join("entrypoint_smoke.o");
    run_command(
        Command::new("clang")
            .args(["-arch", "arm64", "-x", "assembler-with-cpp"])
            .arg(format!("-I{}", generated_dir.display()))
            .arg("-c")
            .arg(root.join("probes/entrypoint_smoke.S"))
            .arg("-o")
            .arg(&object),
    )?;

    let executable = build_dir.join("entrypoint-smoke");
    run_command(
        Command::new("rustc")
            .args(["--edition=2024"])
            .arg(root.join("probes/call_asm.rs"))
            .arg("-C")
            .arg(format!("link-arg={}", object.display()))
            .arg("-o")
            .arg(&executable),
    )?;

    let output = command_output(&mut Command::new(&executable))?;
    if output.trim() != "ART Darwin ARM64 assembly result: 42" {
        return Err(format!("unexpected assembly probe output: {output:?}").into());
    }
    println!("probe-asm: {}", output.trim());
    Ok(())
}

pub(crate) fn isolate_asm_support(mut source: String) -> Result<String> {
    replace_required(
        &mut source,
        "#include \"asm_support_arm64.h\"\n#include \"interpreter/cfi_asm_support.h\"",
        "// Generated ART headers are intentionally reduced by this isolated ABI probe.\n\
#define FRAME_SIZE_SAVE_REFS_ONLY 96\n\
#define FRAME_SIZE_SAVE_REFS_AND_ARGS 224\n\
#define FRAME_SIZE_SAVE_ALL_CALLEE_SAVES 176",
    )?;
    Ok(source)
}

pub(crate) fn probe_page_size(root: &Path) -> Result<()> {
    let source = root.join("_aosp/art/libartbase/base/globals.h");
    if !source.exists() {
        return Err("libartbase source is missing; run `art-bootstrap sync` first".into());
    }

    let build_dir = root.join("_build/page-size-probe");
    let patched_source_dir = build_dir.join("patched-source");
    let patched_header = patched_source_dir.join("libartbase/base/globals.h");
    fs::create_dir_all(patched_header.parent().expect("globals.h has a parent"))?;
    fs::copy(&source, &patched_header)?;
    run_command(
        Command::new("patch")
            .args(["--batch", "--forward", "-p1", "-i"])
            .arg(root.join("patches/art/0002-darwin-dynamic-page-size.patch"))
            .current_dir(&patched_source_dir),
    )?;

    let include_dir = build_dir.join("include/base");
    fs::create_dir_all(&include_dir)?;
    fs::copy(&patched_header, include_dir.join("globals.h"))?;
    fs::write(
        include_dir.join("macros.h"),
        "#pragma once\n#include <unistd.h>\n#define ALWAYS_INLINE __attribute__((always_inline))\n",
    )?;

    let executable = build_dir.join("page-size");
    run_command(
        Command::new("clang++")
            .args(["-std=c++20", "-DART_PAGE_SIZE_AGNOSTIC"])
            .arg(format!("-I{}", build_dir.join("include").display()))
            .arg(root.join("probes/page_size.cc"))
            .arg("-o")
            .arg(&executable),
    )?;

    let output = command_output(&mut Command::new(&executable))?;
    if output.trim() != "ART Darwin page size: 16384" {
        return Err(format!("unexpected page-size probe output: {output:?}").into());
    }
    println!("probe-pagesize: {}", output.trim());
    Ok(())
}

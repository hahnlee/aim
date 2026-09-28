use aim_elf_loader::LoadedElf;
use std::env;
use std::fs;
use std::process::{Command, ExitCode};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let elf_path = arguments.next().ok_or("missing Android ELF fixture")?;
    let native_path = arguments.next().ok_or("missing Mach-O PCS smoke")?;
    if arguments.next().is_some() {
        return Err("usage: android-arm64-pcs-proof <fixture.so> <native-smoke>".into());
    }

    let bytes = fs::read(&elf_path)?;
    let mut image = LoadedElf::load(&bytes)?;
    image.run_initializers()?;
    if image.call_exported_i32("pcs_loader_smoke")? != 42 {
        return Err("ELF loader register-only no-argument proof failed".into());
    }

    let output = Command::new(&native_path).output()?;
    if !output.status.success() {
        return Err(format!(
            "Mach-O PCS proof failed: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let stdout = String::from_utf8(output.stdout)?;
    if !stdout.contains("register-direct=PASS")
        || !stdout.contains("darwin-to-android-spill=PASS")
        || !stdout.contains("android-to-darwin-spill=PASS")
    {
        return Err("Mach-O proof did not report all three acceptance paths".into());
    }
    print!("{stdout}");
    println!("android-arm64-pcs-proof: elf-loader-consumer=PASS fixture=ARM64-Android");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("android-arm64-pcs-proof: {error}");
            ExitCode::from(2)
        }
    }
}

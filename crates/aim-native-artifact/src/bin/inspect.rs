use aim_native_artifact::validate_complete_darwin_macho;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let arguments = env::args_os().collect::<Vec<_>>();
    if arguments.len() != 2 {
        return Err("usage: aim-native-inspect DYLIB".to_owned());
    }
    let path = PathBuf::from(&arguments[1]);
    let bytes =
        fs::read(&path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
    validate_complete_darwin_macho(&bytes).map_err(|error| error.to_string())?;
    println!(
        "native-inspect: PASS format=Mach-O64 architecture=arm64 platform=macos kind=dylib signature=embedded"
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-inspect: {error}");
            ExitCode::from(2)
        }
    }
}

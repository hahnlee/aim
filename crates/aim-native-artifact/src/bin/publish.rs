use aim_native_artifact::{Publication, publish_complete_darwin_graph};
use std::env;
use std::path::Path;
use std::process::ExitCode;

fn run() -> Result<(), String> {
    let arguments = env::args_os().collect::<Vec<_>>();
    if arguments.len() != 6 {
        return Err(
            "usage: aim-native-publish APK_SHA256 RUNTIME_ABI ELF_DIRECTORY STAGING_DIRECTORY DESTINATION"
                .to_owned(),
        );
    }
    let apk_sha256 = arguments[1]
        .to_str()
        .ok_or_else(|| "APK SHA-256 is not UTF-8".to_owned())?;
    let runtime_abi = arguments[2]
        .to_str()
        .ok_or_else(|| "runtime ABI is not UTF-8".to_owned())?;
    let publication = publish_complete_darwin_graph(
        apk_sha256,
        runtime_abi,
        Path::new(&arguments[3]),
        Path::new(&arguments[4]),
        Path::new(&arguments[5]),
    )
    .map_err(|error| error.to_string())?;
    let state = match publication {
        Publication::Published => "published",
        Publication::Existing => "existing",
    };
    println!("native-publish: PASS publication={state}");
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-publish: {error}");
            ExitCode::from(2)
        }
    }
}

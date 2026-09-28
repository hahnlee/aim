use std::env;
use std::fs;
use std::process::ExitCode;

use android_arm64_so_inspect::{error_json, inspect, InspectError};

fn usage() -> ExitCode {
    eprintln!("usage: android-arm64-so-inspect <android-arm64.so>");
    ExitCode::from(64)
}

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    let Some(path) = arguments.next() else {
        return usage();
    };
    if arguments.next().is_some() {
        return usage();
    }
    let display = path.to_string_lossy();
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let error = InspectError {
                code: "io-error",
                message: error.to_string(),
            };
            eprintln!("{}", error_json(Some(&display), &error));
            return ExitCode::from(2);
        }
    };
    match inspect(&bytes) {
        Ok(inspection) => {
            println!("{}", inspection.to_json(Some(&display)));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", error_json(Some(&display), &error));
            ExitCode::from(2)
        }
    }
}

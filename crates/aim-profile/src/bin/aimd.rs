use aim_profile::{DaemonConfig, ProfilePaths, run_daemon};
use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    if let Err(error) = main_result() {
        eprintln!("aimd: {error}");
        std::process::exit(1);
    }
}

fn main_result() -> Result<(), Box<dyn Error>> {
    let mut root = None;
    let mut profile = None;
    let mut idle_seconds = env::var("AIMD_IDLE_SECONDS")
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(300_u64);
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--root") => {
                root = Some(PathBuf::from(
                    arguments.next().ok_or("missing --root value")?,
                ))
            }
            Some("--profile") => {
                profile = Some(
                    arguments
                        .next()
                        .ok_or("missing --profile value")?
                        .into_string()
                        .map_err(|_| "profile must be UTF-8")?,
                )
            }
            Some("--idle-seconds") => {
                idle_seconds = arguments
                    .next()
                    .ok_or("missing --idle-seconds value")?
                    .to_string_lossy()
                    .parse()?
            }
            _ => return Err(format!("unknown argument: {}", argument.to_string_lossy()).into()),
        }
    }
    let paths = match (root, profile) {
        (Some(root), Some(profile)) => ProfilePaths::new(root, &profile)?,
        (None, None) => ProfilePaths::from_environment()?,
        _ => return Err("--root and --profile must be supplied together".into()),
    };
    run_daemon(DaemonConfig {
        paths,
        idle_timeout: Duration::from_secs(idle_seconds),
    })?;
    Ok(())
}

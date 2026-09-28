//! `cargo aim boot`: the boot of docs/boot-status.md on what `cargo aim
//! build` built: aim-display owns the window, guest-init boots the derived
//! image with ANGLE for the GPU. The data directory defaults to
//! `target/aim/boot/data` (never the user's runtime profile). SIGUSR1 to
//! aim-display writes the last presented buffer to
//! `target/aim/boot/capture.bmp`.

use crate::graph::Ctx;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode};
use std::time::{Duration, Instant};

pub fn run(ctx: &Ctx, data: Option<&str>, extra: &[String]) -> Result<ExitCode, String> {
    let dir = aim_paths::out().join("boot");
    let data = data.map_or(dir.join("data"), PathBuf::from);
    fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let mut display = start_display(ctx, &dir)?;
    let status = guest_init(ctx, &data, &dir.join("display"))
        .args(extra)
        .status()
        .map_err(|e| format!("guest-init: {e}"))?;
    // SAFETY: signals the child we started.
    unsafe { libc::kill(display.id() as i32, libc::SIGTERM) };
    let _ = display.wait();
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Starts aim-display with its socket and capture file in `dir` and waits
/// for its socket.
pub fn start_display(ctx: &Ctx, dir: &Path) -> Result<Child, String> {
    let socket = dir.join("display");
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let _ = fs::remove_file(&socket);
    let mut display = Command::new(ctx.workspace.host_bin("aim-display"))
        .args(display_args(dir))
        .spawn()
        .map_err(|e| format!("aim-display: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        if Instant::now() > deadline || display.try_wait().map_err(|e| e.to_string())?.is_some() {
            let _ = display.kill();
            return Err("aim-display did not come up".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(display)
}

/// guest-init with the standard flags of docs/boot-status.md.
pub fn guest_init(ctx: &Ctx, data: &Path, display: &Path) -> Command {
    let mut command = Command::new(ctx.workspace.host_bin("guest-init"));
    command
        .arg("--image")
        .arg(aim_paths::derived_image())
        .arg("--data")
        .arg(data)
        .arg("--run")
        .arg("--gpu")
        .arg(aim_paths::angle())
        .arg("--display")
        .arg(display);
    command
}

/// aim-display's arguments: its socket and capture file in `dir`.
fn display_args(dir: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--socket".into(), dir.join("display").into()];
    args.extend(["--size", "1080x1920", "--capture"].map(OsString::from));
    args.push(dir.join("capture.bmp").into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_captures_next_to_its_socket() {
        let args = display_args(Path::new("/b"));
        let args: Vec<_> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "--socket",
                "/b/display",
                "--size",
                "1080x1920",
                "--capture",
                "/b/capture.bmp"
            ]
        );
    }
}

//! `cargo aim boot`: the boot of docs/boot-status.md on what `cargo aim
//! build` built: aim-display owns the window, guest-init boots the derived
//! image with ANGLE for the GPU. The data directory defaults to
//! `target/aim/boot/data` (never the user's runtime profile).

use crate::graph::Ctx;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

pub fn run(ctx: &Ctx, data: Option<&str>, extra: &[String]) -> Result<ExitCode, String> {
    let dir = aim_paths::out().join("boot");
    let data = data.map_or(dir.join("data"), PathBuf::from);
    let socket = dir.join("display");
    fs::create_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let _ = fs::remove_file(&socket);

    let mut display = Command::new(ctx.workspace.host_bin("aim-display"))
        .arg("--socket")
        .arg(&socket)
        .args(["--size", "1080x1920"])
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

    let status = Command::new(ctx.workspace.host_bin("guest-init"))
        .arg("--image")
        .arg(aim_paths::derived_image())
        .arg("--data")
        .arg(&data)
        .arg("--run")
        .args(["--exclude", "bootanim"])
        .arg("--gpu")
        .arg(aim_paths::angle())
        .arg("--display")
        .arg(&socket)
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

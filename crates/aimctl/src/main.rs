//! `aimctl`: the user's manager of a resident guest (docs/aimctl.md).
//!
//! One guest per data directory runs in the background (`start`, `stop`,
//! `status`), and its apps are listed, installed, opened and uninstalled
//! with Android's own tools run in it (`pm`, `am`, `logcat`). It runs the
//! programs beside it (guest-init, aim-display, aim-apps, linux-run) on
//! what `cargo aim build` built (docs/build.md).

mod args;
mod guest;
mod inputs;
mod output;
mod resident;
mod state;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use args::Command as Cmd;
use guest::Guest;
use state::{Files, State};

/// How long `pm install` may take (dexopt of a large app).
const INSTALL_PATIENCE: Duration = Duration::from_secs(600);
/// How long other framework commands (`pm uninstall`, `am start`) may take.
const COMMAND_PATIENCE: Duration = Duration::from_secs(120);

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match args::parse(&argv).and_then(run) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("aimctl: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The program `name` beside aimctl.
pub fn program(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(name)
}

fn run(a: args::Args) -> Result<ExitCode, String> {
    if a.command == Cmd::Help {
        println!("{}", args::USAGE);
        return Ok(ExitCode::SUCCESS);
    }
    let data = match a.data {
        Some(d) => d,
        None => state::default_data()?,
    };
    let files = Files::of(&data)?;
    match a.command {
        Cmd::Help => unreachable!(),
        Cmd::Start { windows } => resident::start(&files, windows, &a.inputs.resolve()?),
        Cmd::Run { windows } => resident::run(&files, windows, a.inputs.resolve()?),
        Cmd::Stop => resident::stop(&files),
        Cmd::Status => status(&files),
        Cmd::Shell(command) => shell(&files, &command),
        Cmd::Apps => apps(&files),
        Cmd::Install(apks) => install(&files, &apks),
        Cmd::Uninstall(package) => uninstall(&files, &package),
        Cmd::Open(package) => open(&files, &package),
        Cmd::Logs { follow, args } => logs(&files, follow, &args),
    }
}

/// The running guest, or why there is none to talk to.
fn running(files: &Files) -> Result<(State, Guest), String> {
    let state = state::resident(files)?
        .ok_or_else(|| format!("{}: not running; `aimctl start`", files.data.display()))?;
    let guest = Guest::of(files, &state)
        .ok_or_else(|| format!("{}: the guest is starting; try again", files.data.display()))?;
    Ok((state, guest))
}

/// Like [`running`], and Android finished booting.
fn booted(files: &Files) -> Result<Guest, String> {
    let (_, guest) = running(files)?;
    if !guest.booted() {
        return Err("Android is still booting; `aimctl status` shows when it is ready".into());
    }
    Ok(guest)
}

fn bytes(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.2} GB", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.0} MB", n as f64 / 1e6),
        n => format!("{:.0} kB", n as f64 / 1e3),
    }
}

fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m {s:02}s"),
        _ => format!("{h}h {m:02}m {s:02}s"),
    }
}

fn status(files: &Files) -> Result<ExitCode, String> {
    let resident = state::resident(files)?;
    let state = match &resident {
        None => "stopped",
        Some(s) => match Guest::of(files, s) {
            None => "starting",
            Some(g) if g.booted() => "running",
            Some(_) => "booting",
        },
    };
    println!("state:   {state}");
    println!("data:    {}", files.data.display());
    if let Some(s) = &resident {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mode = if s.windows {
            "window mode"
        } else {
            "device mode"
        };
        println!("pid:     {} ({mode})", s.pid);
        println!("uptime:  {}", duration(now.saturating_sub(s.started)));
        if let Some(pid) = s.guest {
            let u = guest::measure(files, pid);
            println!(
                "guest:   {} processes, CPU {:.0}%, memory {}",
                u.processes,
                u.cpu,
                bytes(u.rss_kib * 1024)
            );
        }
    }
    if aim_storage::data::image_of(&files.data).exists() {
        let u = aim_storage::data::usage(&files.data)?;
        let used = u.used.map_or(String::new(), |b| {
            format!(", {} used by its files", bytes(b))
        });
        println!(
            "image:   {} ({} on disk{used})",
            aim_storage::data::image_of(&files.data).display(),
            bytes(u.allocated)
        );
    }
    println!("log:     {}", files.log().display());
    Ok(ExitCode::SUCCESS)
}

/// The exit code of a guest program (a signal's is 128 + its number).
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    let code = status
        .code()
        .or(status.signal().map(|s| 128 + s))
        .unwrap_or(1);
    ExitCode::from(code as u8)
}

fn shell(files: &Files, command: &[String]) -> Result<ExitCode, String> {
    let (_, guest) = running(files)?;
    let script = command.join(" ");
    let argv: Vec<&str> = if command.is_empty() {
        vec!["/system/bin/sh"]
    } else {
        vec!["/system/bin/sh", "-c", &script]
    };
    let mut command = guest.command(&argv);
    // An interactive shell gets the terminal's type, as adbd's does.
    if let (true, Some(term)) = (argv.len() == 1, std::env::var_os("TERM")) {
        command.env("TERM", term);
    }
    let status = command.status().map_err(|e| format!("linux-run: {e}"))?;
    Ok(exit_code(status))
}

/// The launcher apps of the running guest, as its image and `/data` hold
/// them.
fn launcher_apps(files: &Files) -> Result<Vec<aim_apps::installed::Installed>, String> {
    let (state, _) = running(files)?;
    let root = state.inputs.image;
    let framework_res = root.join("system/framework/framework-res.apk");
    let framework = aim_apps::apk::Apk::open(&framework_res)
        .map_err(|e| format!("{}: {e}", framework_res.display()))?;
    Ok(aim_apps::installed::scan(
        &root,
        Some(&files.guest_data()),
        &framework,
    ))
}

fn apps(files: &Files) -> Result<ExitCode, String> {
    let apps = launcher_apps(files)?;
    let width = apps.iter().map(|i| i.app.package.len()).max().unwrap_or(0);
    for i in &apps {
        println!("{:width$}  {}", i.app.package, i.app.label);
    }
    Ok(ExitCode::SUCCESS)
}

/// pm's verdict: its output, and whether it says "Success".
fn pm(guest: &Guest, argv: &[&str], patience: Duration) -> Result<ExitCode, String> {
    let (_, out) = guest.output(argv, patience)?;
    let out = out.trim();
    if out.lines().any(|l| l.trim() == "Success") {
        println!("{out}");
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!("{}: {out}", argv[..2].join(" ")))
    }
}

fn install(files: &Files, apks: &[PathBuf]) -> Result<ExitCode, String> {
    let guest = booted(files)?;
    let tmp = files.guest_data().join("local/tmp");
    // Copies (APFS clones) in the guest's /data/local/tmp, which pm reads;
    // the APKs themselves are never changed.
    let mut copies = Vec::new();
    let result = (|| {
        for (n, apk) in apks.iter().enumerate() {
            if !apk.is_file() {
                return Err(format!("{}: not a file", apk.display()));
            }
            let name = format!("aimctl-{}-{n}.apk", std::process::id());
            let copy = tmp.join(&name);
            fs::copy(apk, &copy).map_err(|e| format!("{}: {e}", apk.display()))?;
            copies.push(copy);
            fs::set_permissions(copies.last().unwrap(), fs::Permissions::from_mode(0o644))
                .map_err(|e| format!("{name}: {e}"))?;
        }
        let guest_paths: Vec<String> = copies
            .iter()
            .map(|c| {
                format!(
                    "/data/local/tmp/{}",
                    c.file_name().unwrap().to_string_lossy()
                )
            })
            .collect();
        let mut argv = vec!["/system/bin/pm", "install", "-r"];
        argv.extend(guest_paths.iter().map(String::as_str));
        pm(&guest, &argv, INSTALL_PATIENCE)
    })();
    for copy in &copies {
        let _ = fs::remove_file(copy);
    }
    result
}

fn uninstall(files: &Files, package: &str) -> Result<ExitCode, String> {
    let guest = booted(files)?;
    pm(
        &guest,
        &["/system/bin/pm", "uninstall", package],
        COMMAND_PATIENCE,
    )
}

/// The shim in `dir` for `package`.
fn shim_of(dir: &Path, package: &str) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| aim_apps::shim::stamp(p).is_some_and(|s| s.package == package))
}

fn open(files: &Files, package: &str) -> Result<ExitCode, String> {
    let (state, _) = running(files)?;
    let app = launcher_apps(files)?
        .into_iter()
        .find(|i| i.app.package == package)
        .ok_or_else(|| format!("{package}: not a launcher app of the guest (`aimctl apps`)"))?;
    // In window mode the app's shim hosts its windows under its own Dock
    // icon, and opening it launches the app.
    if state.windows
        && let Some(shim) = shim_of(&files.apps(), package)
    {
        let status = Command::new("/usr/bin/open")
            .arg(&shim)
            .status()
            .map_err(|e| format!("open: {e}"))?;
        return Ok(exit_code(status));
    }
    let guest = booted(files)?;
    let component = format!("{package}/{}", app.app.activity);
    let (status, out) = guest.output(
        &["/system/bin/am", "start", "-W", "-n", &component],
        COMMAND_PATIENCE,
    )?;
    print!("{out}");
    Ok(exit_code(status))
}

fn logs(files: &Files, follow: bool, extra: &[String]) -> Result<ExitCode, String> {
    let (_, guest) = running(files)?;
    let mut argv = vec!["/system/bin/logcat"];
    if !follow {
        argv.push("-d");
    }
    argv.extend(extra.iter().map(String::as_str));
    let status = guest
        .command(&argv)
        .status()
        .map_err(|e| format!("linux-run: {e}"))?;
    Ok(exit_code(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_durations() {
        assert_eq!(bytes(2_860_000_000), "2.86 GB");
        assert_eq!(bytes(21_000_000), "21 MB");
        assert_eq!(bytes(4096), "4 kB");
        assert_eq!(duration(12), "12s");
        assert_eq!(duration(245), "4m 05s");
        assert_eq!(duration(3723), "1h 02m 03s");
    }

    #[test]
    fn a_shim_is_found_by_its_package() {
        let dir = std::env::temp_dir().join(format!("aimctl-shims-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (name, package) in [
            ("Settings", "com.android.settings"),
            ("Chrome", "com.android.chrome"),
        ] {
            let contents = dir.join(format!("{name}.app/Contents"));
            fs::create_dir_all(&contents).unwrap();
            fs::write(
                contents.join("Info.plist"),
                format!("<dict>\n\t<key>AIMPackage</key>\n\t<string>{package}</string>\n</dict>\n"),
            )
            .unwrap();
        }
        assert_eq!(
            shim_of(&dir, "com.android.chrome"),
            Some(dir.join("Chrome.app"))
        );
        assert_eq!(shim_of(&dir, "com.example"), None);
        assert_eq!(shim_of(&dir.join("none"), "com.android.chrome"), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}

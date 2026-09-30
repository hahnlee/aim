//! `aim-apps`: the guest's launcher apps as macOS apps.
//!
//! ```text
//! aim-apps shims --image ROOT [--data DATA] --display SOCKET --host AIM_DISPLAY --into DIR [--watch]
//! aim-apps install --image ROOT [--data DATA] --display SOCKET --host AIM_DISPLAY
//! aim-apps clean --into DIR
//! aim-apps icon APK --framework FRAMEWORK_RES --out FILE.png|FILE.icns [--size N]
//! ```
//!
//! - `shims` keeps `DIR` holding one shim per launcher activity of the guest
//!   whose system image root is `ROOT` and whose `/data` is `DATA`, and
//!   one for the platform (`android`, which shows the notifications of
//!   packages without a shim of their own); with
//!   `--watch` it follows installs and uninstalls (`DATA/system/
//!   packages.list` changing), and a rebuilt `AIM_DISPLAY`, until it is
//!   stopped.
//! - `install` does the same into `~/Applications/aim Apps`.
//! - `clean` removes the shims from `DIR` and from Launch Services.
//! - `icon` lists an APK's launcher activities and draws the first one's
//!   icon as a macOS icon.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use aim_apps::apk::{Apk, Resources};
use aim_apps::icon::{self, Icon};

const USAGE: &str = "usage:
  aim-apps shims --image ROOT [--data DATA] --display SOCKET --host AIM_DISPLAY --into DIR [--watch]
  aim-apps install --image ROOT [--data DATA] --display SOCKET --host AIM_DISPLAY
  aim-apps clean --into DIR
  aim-apps icon APK --framework FRAMEWORK_RES --out FILE.png|FILE.icns [--size N]";

/// How often `--watch` looks at the package list.
const POLL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Args {
    positional: Vec<PathBuf>,
    framework: Option<PathBuf>,
    out: Option<PathBuf>,
    size: Option<usize>,
    image: Option<PathBuf>,
    data: Option<PathBuf>,
    display: Option<PathBuf>,
    host: Option<PathBuf>,
    into: Option<PathBuf>,
    watch: bool,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aim-apps: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(rest: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().map(PathBuf::from).ok_or(USAGE.to_string());
        match arg.as_str() {
            "--framework" => a.framework = Some(value()?),
            "--out" => a.out = Some(value()?),
            "--size" => {
                a.size = Some(
                    value()?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| USAGE.to_string())?,
                )
            }
            "--image" => a.image = Some(value()?),
            "--data" => a.data = Some(value()?),
            "--display" => a.display = Some(value()?),
            "--host" => a.host = Some(value()?),
            "--into" => a.into = Some(value()?),
            "--watch" => a.watch = true,
            s if s.starts_with("--") => return Err(USAGE.into()),
            _ => a.positional.push(PathBuf::from(arg)),
        }
    }
    Ok(a)
}

fn run(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let a = parse(rest)?;
    match cmd.as_str() {
        "icon" => icon_command(&a),
        "shims" => shims(&a, a.into.clone().ok_or(USAGE)?),
        "install" => {
            let home = std::env::var_os("HOME").ok_or("no HOME")?;
            shims(&a, Path::new(&home).join("Applications/aim Apps"))
        }
        "clean" => {
            let dir = a.into.as_deref().ok_or(USAGE)?;
            for p in aim_apps::shim::clean(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                println!("removed {p}");
            }
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

fn open(path: &Path) -> Result<Apk, String> {
    Apk::open(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn icon_command(a: &Args) -> Result<(), String> {
    let [apk] = a.positional.as_slice() else {
        return Err(USAGE.into());
    };
    let out = a.out.as_ref().ok_or(USAGE)?;
    let apk = open(apk)?;
    let framework = a.framework.as_deref().map(open).transpose()?;
    let apps = aim_apps::app::read(&apk, framework.as_ref(), None).map_err(|e| e.to_string())?;
    for app in &apps {
        println!(
            "{}/{} \"{}\" version {}",
            app.package, app.activity, app.label, app.version
        );
    }
    let app = apps.first().ok_or("not a launcher app")?;
    let bytes = if out.extension().is_some_and(|e| e == "icns") {
        app.icns(&apk, framework.as_ref())
    } else {
        let res = Resources {
            app: &apk,
            framework: framework.as_ref(),
            theme: app.theme,
        };
        let icon = Icon::of(&res, app.icon.as_ref().ok_or("no icon")?).ok_or("no icon")?;
        icon::render(&res, &icon, a.size.unwrap_or(1024)).png()
    }
    .ok_or("the icon could not be drawn")?;
    std::fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn shims(a: &Args, dir: PathBuf) -> Result<(), String> {
    let root = a.image.as_deref().ok_or(USAGE)?;
    let socket = a.display.as_deref().ok_or(USAGE)?;
    let host = a.host.as_deref().ok_or(USAGE)?;
    let framework = open(&root.join("system/framework/framework-res.apk"))?;
    let data = a.data.as_deref();
    let mut seen = None;
    loop {
        // The guest's package list, and the window host binary.
        let now = (
            data.and_then(|d| modified(&d.join("system/packages.list"))),
            modified(host),
        );
        if seen != Some(now) {
            let mut apps = aim_apps::installed::scan(root, data, &framework);
            // The platform's own shim shows notifications of packages
            // without one.
            let framework_path = root.join("system/framework/framework-res.apk");
            if let Ok(Some(app)) = aim_apps::app::system(&framework) {
                apps.push(aim_apps::installed::Installed {
                    apk: framework_path,
                    app,
                });
            }
            let done = aim_apps::shim::sync(&dir, &apps, &framework, socket, host)
                .map_err(|e| format!("{}: {e}", dir.display()))?;
            for p in &done.written {
                println!("shim {p}");
            }
            for p in &done.removed {
                println!("removed {p}");
            }
            println!("{} shims in {}", apps.len(), dir.display());
            seen = Some(now);
        }
        if !a.watch {
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

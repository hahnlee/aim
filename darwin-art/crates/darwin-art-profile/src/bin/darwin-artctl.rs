use darwin_art_profile::{
    ProfileLease, ProfilePaths, create_profile, daemon_status, daemonize_process, delete_profile,
    ensure_daemon, list_packages, list_processes, list_profile_ids, profile_allocated_bytes,
    register_package, resolve_package, shutdown_daemon, unregister_package,
};
use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn main() {
    if let Err(error) = main_result() {
        eprintln!("darwin-artctl: {error}");
        std::process::exit(1);
    }
}

fn main_result() -> Result<(), Box<dyn Error>> {
    let paths = ProfilePaths::from_environment()?;
    match env::args().nth(1).as_deref() {
        Some("ensure") => {
            let path = ensure_daemon(&paths)?;
            io::stdout().write_all(path.as_os_str().as_encoded_bytes())?;
            println!();
        }
        Some("socket") => {
            io::stdout().write_all(paths.socket.as_os_str().as_encoded_bytes())?;
            println!();
        }
        Some("status") => println!("{}", daemon_status(&paths)?),
        Some("process-identity") => {
            let pid: u32 = env::args()
                .nth(2)
                .ok_or("process-identity requires PID")?
                .parse()?;
            let identity = darwin_art_profile::resolve_process_identity(&paths, pid)?;
            println!("{}\t{}\t{}", identity.pid, identity.uid, identity.package);
        }
        Some("profiles") => {
            for profile in list_profile_ids(&paths.profiles_root)? {
                println!("{profile}");
            }
        }
        Some("create-profile") => {
            let profile = env::args().nth(2).ok_or("create-profile requires id")?;
            create_profile(&paths.profiles_root, &profile)?;
        }
        Some("delete-profile") => {
            let profile = env::args().nth(2).ok_or("delete-profile requires id")?;
            delete_profile(&paths.profiles_root, &profile)?;
        }
        Some("profile-size") => {
            let profile = env::args().nth(2).ok_or("profile-size requires id")?;
            println!(
                "{}",
                profile_allocated_bytes(&paths.profiles_root, &profile)?
            );
        }
        Some("shutdown") => shutdown_daemon(&paths)?,
        Some("register") => {
            let package = env::args().nth(2).ok_or("register requires package")?;
            let record = env::args_os()
                .nth(3)
                .ok_or("register requires record file")?;
            register_package(&paths, &package, &fs::read(record)?)?;
        }
        Some("resolve") => {
            let package = env::args().nth(2).ok_or("resolve requires package")?;
            io::stdout().write_all(&resolve_package(&paths, &package)?)?;
        }
        Some("uninstall") => {
            let package = env::args().nth(2).ok_or("uninstall requires package")?;
            let keep_data = env::args()
                .nth(3)
                .is_some_and(|value| value == "--keep-data");
            unregister_package(&paths, &package, !keep_data)?;
        }
        Some("list") => print!("{}", list_packages(&paths)?),
        Some("ps") => print!("{}", list_processes(&paths)?),
        Some("hold") => {
            let seconds = env::args().nth(2).ok_or("hold requires seconds")?.parse()?;
            let _lease = ProfileLease::connect(&paths.socket)?;
            thread::sleep(Duration::from_secs_f64(seconds));
        }
        Some("supervise") => {
            let mut arguments = env::args_os().skip(2);
            let package = arguments
                .next()
                .ok_or("supervise requires package")?
                .into_string()
                .map_err(|_| "package is not UTF-8")?;
            let program = arguments.next().ok_or("supervise requires command")?;
            let mut child = darwin_art_profile::spawn_owned(Command::new(program).args(arguments))?;
            let lease = match ProfileLease::connect_process_pid(&paths.socket, child.id(), &package)
            {
                Ok(lease) => lease,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            };
            let status = child.wait()?;
            drop(lease);
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
        }
        Some("start-runtime") => {
            let arguments = env::args_os().skip(2).collect::<Vec<_>>();
            let response = darwin_art_profile::runtime_service_cli::start_runtime_cli(
                &paths.socket,
                &arguments,
                launch_environment(),
            )?;
            println!("{}", response.pid);
        }
        Some("daemonize") => {
            let mut arguments = env::args_os().skip(2);
            let package = arguments
                .next()
                .ok_or("daemonize requires package")?
                .into_string()
                .map_err(|_| "package is not UTF-8")?;
            let arguments = arguments.collect::<Vec<_>>();
            let environment = launch_environment();
            println!(
                "{}",
                daemonize_process(&paths, &package, &arguments, &environment)?
            );
        }
        Some("exec") => {
            let mut arguments = env::args_os().skip(2);
            let package = arguments
                .next()
                .ok_or("exec requires package")?
                .into_string()
                .map_err(|_| "package is not UTF-8")?;
            let program = arguments.next().ok_or("exec requires command")?;
            let lease = ProfileLease::connect_process(&paths.socket, &package)?;
            let lease_fd = lease.preserve_for_exec()?;
            return Err(Command::new(program)
                .args(arguments)
                .env("DARWIN_ART_PROFILE_LEASE_FD", lease_fd.to_string())
                .exec()
                .into());
        }
        _ => {
            return Err("usage: darwin-artctl {ensure|socket|status|profiles|create-profile ID|delete-profile ID|profile-size ID|shutdown|register PACKAGE RECORD|resolve PACKAGE|uninstall PACKAGE [--keep-data]|list|ps|hold SECONDS|supervise PACKAGE COMMAND [ARGS...]|daemonize PACKAGE COMMAND [ARGS...]|start-runtime PACKAGE KEY_HEX REQUIRED_MASK COMMAND [ARGS...]|exec PACKAGE COMMAND [ARGS...]}".into());
        }
    }
    Ok(())
}

fn launch_environment() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    env::vars_os()
        .filter(|(key, _)| {
            let key = key.to_string_lossy();
            key.starts_with("DARWIN_ART_")
                || key.starts_with("ANDROID_")
                || matches!(
                    key.as_ref(),
                    "PATH" | "HOME" | "TMPDIR" | "LANG" | "LC_ALL" | "SHELL"
                )
        })
        .collect()
}

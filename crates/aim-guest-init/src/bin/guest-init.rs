//! `guest-init --image <derived-root> --data <writable-root> --dry-run|--run
//! [--only svc1,svc2] [--exclude svc1,svc2] [--runtime DIR] [--linux-run PATH]
//! [--gpu DIR] [--display SOCKET] [--trace]
//! [--timeout SECS] [--androidboot KEY=VALUE]... [--quiet]`
//!
//! Development entry point for aimd's init role. With `--run`, the data
//! directory's persistent content lives in a case-sensitive disk image
//! beside it (`<data>.asif`), created on first use, attached hidden at
//! `<data>` for the boot and detached when it stops (docs/storage.md).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use aim_guest_init::{Boot, BootOptions, RunMode, boot};

fn usage() -> ! {
    eprintln!(
        "usage: guest-init --image DIR --data DIR (--dry-run | --run) [--only a,b] [--exclude a,b] [--runtime DIR]\n\
         \x20                 [--linux-run PATH] [--gpu DIR] [--display SOCKET] [--trace]\n\
         \x20                 [--timeout SECS] [--androidboot KEY=VALUE]... [--quiet]"
    );
    std::process::exit(2);
}

extern "C" fn stop(_: libc::c_int) {
    boot::request_stop();
}

fn main() {
    // SIGINT/SIGTERM stop the services before exiting, as a timeout does.
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler only stores to an atomic.
        unsafe {
            libc::signal(
                signal,
                stop as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
    let mut args = std::env::args().skip(1);
    let (mut image, mut data, mut mode) = (None, None, None);
    let mut runtime = None;
    let mut only = None;
    let mut exclude = BTreeSet::new();
    let mut linux_run = None;
    let (mut gpu, mut display) = (None, None);
    let mut trace = false;
    let mut timeout = None;
    let mut androidboot = Vec::new();
    let mut quiet = false;
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--image" => image = Some(PathBuf::from(value())),
            "--data" => data = Some(PathBuf::from(value())),
            "--runtime" => runtime = Some(PathBuf::from(value())),
            "--dry-run" => mode = Some(RunMode::DryRun),
            "--run" => mode = Some(RunMode::Run),
            "--only" => {
                only = Some(
                    value()
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect::<BTreeSet<_>>(),
                )
            }
            "--exclude" => exclude.extend(
                value()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            ),
            "--linux-run" => linux_run = Some(PathBuf::from(value())),
            "--gpu" => gpu = Some(PathBuf::from(value())),
            "--display" => display = Some(PathBuf::from(value())),
            "--trace" => trace = true,
            "--quiet" => quiet = true,
            "--timeout" => {
                timeout = Some(Duration::from_secs_f64(
                    value().parse().unwrap_or_else(|_| usage()),
                ))
            }
            "--androidboot" => {
                let entry = value();
                let (k, v) = entry.split_once('=').unwrap_or_else(|| usage());
                androidboot.push((k.to_string(), v.to_string()));
            }
            _ => usage(),
        }
    }
    let (Some(image), Some(data), Some(mode)) = (image, data, mode) else {
        usage()
    };
    let mut options = BootOptions::new(image, data, mode);
    options.runtime = runtime;
    options.only = only;
    options.exclude = exclude;
    options.linux_run = linux_run;
    options.gpu = gpu;
    options.display = display;
    options.trace = trace;
    options.timeout = timeout;
    if !androidboot.is_empty() {
        options.androidboot = androidboot;
    }
    let mut boot = match Boot::prepare(options) {
        Ok(boot) => boot,
        Err(error) => {
            eprintln!("guest-init: {error}");
            std::process::exit(1);
        }
    };
    let report = boot.run().clone();
    if !quiet {
        for diagnostic in &report.diagnostics {
            println!("diag: {diagnostic}");
        }
        for command in &report.commands {
            let executed = &command.executed;
            let result = match &executed.result {
                Ok(()) => String::new(),
                Err(error) => format!("  ERROR: {error}"),
            };
            println!(
                "[{}] {}:{} {}{}",
                executed.action, executed.file, executed.line, executed.command, result
            );
            for effect in &command.effects {
                println!("    {:<8} {}", effect.kind(), effect.text());
                if let aim_guest_init::fsops::Effect::Launch { pid, .. } = effect
                    && let Some((_, description)) = report.launches.iter().find(|(p, _)| p == pid)
                {
                    for line in description.lines() {
                        println!("      {line}");
                    }
                }
            }
        }
        for simulated in &report.simulated {
            println!("simulated: {simulated}");
        }
        for line in &report.log {
            println!("log: {line}");
        }
        println!("no-op reasons:");
        for (reason, count) in report.noop_reasons() {
            println!("  {count:>4}  {reason}");
        }
    }
    println!("triggers: {}", report.triggers.join(" "));
    println!("{}", report.summary());
    if report.fatal.is_some() {
        std::process::exit(1);
    }
}

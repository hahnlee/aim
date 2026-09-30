//! `guest-init --image <derived-root> --data <writable-root> --dry-run|--run
//! [--only svc1,svc2] [--exclude svc1,svc2] [--runtime DIR] [--linux-run PATH]
//! [--gpu DIR] [--vulkan DIR] [--display SOCKET] [--trace]
//! [--binder-trace FILE] [--timeout SECS] [--androidboot KEY=VALUE]... [--quiet]
//! [--userdata DIR]`
//!
//! Development entry point for aimd's init role. With `--run`, the data
//! directory's persistent content lives in a case-sensitive disk image
//! beside it (`<data>.asif`), created on first use (a copy of a template
//! in the `userdata` node's output `--userdata DIR`, where there is one),
//! attached hidden at `<data>` for the boot and detached when it stops
//! (docs/storage.md).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use aim_guest_init::{Boot, BootOptions, RunMode, boot};

fn usage() -> ! {
    eprintln!(
        "usage: guest-init --image DIR --data DIR (--dry-run | --run) [--only a,b] [--exclude a,b] [--runtime DIR]\n\
         \x20                 [--linux-run PATH] [--gpu DIR] [--vulkan DIR] [--display SOCKET] [--trace]\n\
         \x20                 [--binder-trace FILE] [--timeout SECS] [--androidboot KEY=VALUE]... [--quiet]\n\
         \x20                 [--userdata DIR]"
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
    let (mut gpu, mut vulkan, mut display) = (None, None, None);
    let mut trace = false;
    let mut binder_trace = None;
    let mut timeout = None;
    let mut androidboot = Vec::new();
    let mut quiet = false;
    let mut userdata = None;
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
            "--vulkan" => vulkan = Some(PathBuf::from(value())),
            "--display" => display = Some(PathBuf::from(value())),
            "--trace" => trace = true,
            "--binder-trace" => binder_trace = Some(PathBuf::from(value())),
            "--quiet" => quiet = true,
            "--userdata" => userdata = Some(PathBuf::from(value())),
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
    options.vulkan = vulkan;
    options.display = display;
    options.trace = trace;
    options.binder_trace = binder_trace;
    options.timeout = timeout;
    options.userdata = userdata;
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
    // Detaches the data image; `exit` below would skip it.
    drop(boot);
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
                "{:8.3} [{}] {}:{} {}{}",
                command.at.as_secs_f64(),
                executed.action,
                executed.file,
                executed.line,
                executed.command,
                result
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
        for (at, what) in &report.timeline {
            println!("timeline: {:8.3} {what}", at.as_secs_f64());
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

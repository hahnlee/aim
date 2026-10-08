//! `cargo aim`: the repository's build graph (docs/build.md).
//!
//! Every build step is a node with declared inputs, upstream nodes and
//! outputs, and a key over their content; a node whose key matches its
//! stamp is skipped, and within a node cargo and n2 do their own
//! incremental work. The cargo nodes come from `cargo metadata`, the
//! derived image's upstream from `image/overlay.toml`.

mod bench;
mod boot;
mod cargo;
mod fetch;
mod graph;
mod hash;
mod lockfile;
mod log;
mod n2db;
mod nodes;
mod stamp;
mod storage;
mod test;
mod tools;

use graph::{Ctx, Graph, Options};
use std::ffi::OsStr;
use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
usage: cargo aim <command> [options]

commands:
  build [NODE...]        build NODEs and what they need (default: what a boot needs)
  test [--integration]   build what the tests need, then run each test binary
       [--timeout SECS]  with a timeout (default 900); --integration adds the
                         tests/ targets and builds every node first. A test
                         that skips for a missing input fails.
  boot [--data DIR] [--windows] [-- GUEST-INIT-ARGS...]
                         build, then run aim-display and guest-init with the
                         standard flags of docs/boot-status.md; --windows
                         shows each app in its own window (docs/windows.md)
  bench [--runs N] [--keep]
                         build, then boot fresh data directories N times
                         (default 1) and measure boot, process creation,
                         binder, app starts, Chrome and memory; writes
                         target/aim/bench/<timestamp>.json (docs/perf-baseline.md)
  bench --compare A.json B.json
                         the change of every median from A to B
  template --output NEWDIR --image EXISTING_ROOT --host-runtime BIN_DIR
           --display-bin FILE --empty-template FILE
                         isolated first-boot template from existing inputs;
                         no graph build and no shared template replacement
  check-service-java DEX STUB_DEX
                         verify linkage and generated transaction codes; no build
  generate-service-aidl regenerate the typed service contract from existing pins;
                         no image or dependency build
  status [NODE...]       which nodes are stale, and why
  storage [DATA...]      what the system, derived and data images occupy on the
                         host, and what a data image could give back (docs/storage.md)
  clean [NODE...]        forget NODEs (default: all) and remove their outputs

options:
  -v, --verbose          show the commands' output
  -j, --jobs N           nodes to run at once (default 3)

A NODE is a name from `cargo aim status` or a prefix of names: `hal` is every
hal/... node.";

fn main() -> ExitCode {
    let mut args = std::env::args_os();
    // The ninja nodes run this binary as n2.
    if args
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_name)
        == Some(OsStr::new(n2db::ARGV0))
    {
        return match n2::run::run() {
            Ok(0) => ExitCode::SUCCESS,
            Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            Err(error) => {
                eprintln!("n2: error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let args: Vec<String> = args.map(|a| a.to_string_lossy().into_owned()).collect();
    match run(args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("cargo aim: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Args {
    command: String,
    names: Vec<String>,
    verbose: bool,
    jobs: usize,
    integration: bool,
    timeout: Duration,
    data: Option<String>,
    rest: Vec<String>,
    windows: bool,
    runs: usize,
    keep: bool,
    compare: Vec<String>,
    template_inputs: std::collections::HashMap<String, String>,
}

fn parse(args: Vec<String>) -> Result<Args, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or(USAGE)?;
    let mut parsed = Args {
        command,
        names: Vec::new(),
        verbose: false,
        jobs: 3,
        integration: false,
        timeout: Duration::from_secs(900),
        data: None,
        rest: Vec::new(),
        windows: false,
        runs: 1,
        keep: false,
        compare: Vec::new(),
        template_inputs: std::collections::HashMap::new(),
    };
    while let Some(arg) = args.next() {
        let mut value = |flag: &str| args.next().ok_or(format!("{flag} needs a value"));
        match arg.as_str() {
            "--output" | "--image" | "--host-runtime" | "--display-bin" | "--empty-template"
                if parsed.command == "template" =>
            {
                if parsed
                    .template_inputs
                    .insert(arg.clone(), value(&arg)?)
                    .is_some()
                {
                    return Err(format!("duplicate {arg}"));
                }
            }
            "-v" | "--verbose" => parsed.verbose = true,
            "-j" | "--jobs" => {
                parsed.jobs = value(&arg)?.parse().map_err(|_| "--jobs needs a number")?
            }
            "--integration" if parsed.command == "test" => parsed.integration = true,
            "--timeout" if parsed.command == "test" => {
                let secs = value(&arg)?
                    .parse()
                    .map_err(|_| "--timeout needs seconds")?;
                parsed.timeout = Duration::from_secs(secs);
            }
            "--data" if parsed.command == "boot" => parsed.data = Some(value(&arg)?),
            "--windows" if parsed.command == "boot" => parsed.windows = true,
            "--runs" if parsed.command == "bench" => {
                parsed.runs = value(&arg)?
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or("--runs needs a positive number")?
            }
            "--keep" if parsed.command == "bench" => parsed.keep = true,
            "--compare" if parsed.command == "bench" => {
                parsed.compare = vec![value(&arg)?, value(&arg)?];
            }
            "--" if parsed.command == "boot" => {
                parsed.rest = args.by_ref().collect();
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            name if matches!(
                parsed.command.as_str(),
                "build" | "status" | "clean" | "storage" | "check-service-java"
            ) =>
            {
                parsed.names.push(name.into())
            }
            other => return Err(format!("unexpected argument {other}\n\n{USAGE}")),
        }
    }
    Ok(parsed)
}

fn load(verbose: bool) -> Result<(Graph, Ctx), String> {
    let workspace = cargo::Workspace::load()?;
    let nodes = nodes::declare(workspace.nodes()?)?;
    let graph = Graph::new(nodes)?;
    Ok((
        graph,
        Ctx {
            tools: tools::Tools::default(),
            workspace,
            verbose,
        },
    ))
}

/// Holds `target/aim-cache/lock` while this process builds: two builds of
/// one checkout would race on its stamps and outputs.
fn lock() -> Result<fs::File, String> {
    let path = aim_paths::cache().join("lock");
    fs::create_dir_all(aim_paths::cache()).map_err(|e| e.to_string())?;
    let file = fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: flock on an open descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        eprintln!("cargo aim: waiting for another cargo aim in this checkout");
        // SAFETY: as above.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!(
                "{}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(file)
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let args = parse(args)?;
    let options = Options { jobs: args.jobs };
    match args.command.as_str() {
        "check-service-java" => {
            if args.names.len() != 2 { return Err("check-service-java requires DEX STUB_DEX".into()); }
            let image = aim_paths::original_image();
            let _image_lease = aim_storage::system::ImageLease::read_root(&image)?;
            use aim_android_image::classpath::{self, BOOTCLASSPATH, SYSTEMSERVERCLASSPATH};
            let mut jars = classpath::jars(&image, "bootclasspath.pb", BOOTCLASSPATH)?;
            jars.extend(classpath::jars(&image, "systemserverclasspath.pb", SYSTEMSERVERCLASSPATH)?);
            nodes::java::check_linkage(&image, &jars, Some(Path::new(&args.names[1])), Path::new(&args.names[0]))?;
            nodes::service_aidl::check_own_stubs(Path::new(&args.names[0]))?;
            Ok(ExitCode::SUCCESS)
        }
        "generate-service-aidl" => {
            if !args.names.is_empty() { return Err("generate-service-aidl takes no targets".into()); }
            let _lock = lock()?;
            let image = aim_paths::original_image();
            let _image_lease = aim_storage::system::ImageLease::read_root(&image)?;
            let lock_path = aim_paths::root().join(nodes::service_aidl::LOCK);
            let inputs = || -> Result<Vec<(String, String)>, String> {
                let contract = lockfile::Lock::read(&lock_path)?;
                let mut paths = vec![lock_path.clone()];
                for entry in contract.array("OWN_INTERFACES") {
                    let file = entry.split('|').nth(1).ok_or("invalid own-interface input")?;
                    paths.push(aim_paths::root().join(file));
                }
                paths.into_iter().map(|path| {
                    let hash = hash::sha256_file(&path).map_err(|error| error.to_string())?;
                    Ok((path.display().to_string(), hash))
                }).collect()
            };
            let before = inputs()?;
            let mut log = log::Log::create(aim_paths::cache().join("logs/service-aidl-direct.log"), args.verbose)?;
            nodes::service_aidl::run(&mut log)?;
            if inputs()? != before { return Err("service AIDL inputs changed during generation".into()); }
            let output = nodes::service_aidl::out().join("lib.rs");
            let receipt = serde_json::json!({
                "original_image": image, "inputs": before,
                "output": output, "output_sha256": hash::sha256_file(&output).map_err(|error| error.to_string())?
            });
            fs::write(nodes::service_aidl::out().join("direct-provenance.json"),
                serde_json::to_vec_pretty(&receipt).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
            Ok(ExitCode::SUCCESS)
        }
        "template" => {
            let input = |name: &str| {
                args.template_inputs
                    .get(name)
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("template requires {name}"))
            };
            let config = nodes::userdata::IsolatedTemplate {
                output: input("--output")?,
                image: input("--image")?,
                host_runtime: input("--host-runtime")?,
                display: input("--display-bin")?,
                empty: input("--empty-template")?,
            };
            let (_, ctx) = load(args.verbose)?;
            nodes::userdata::run_isolated_template(&ctx, &config)?;
            Ok(ExitCode::SUCCESS)
        }
        "build" => {
            let (graph, ctx) = load(args.verbose)?;
            let targets = if args.names.is_empty() {
                graph.boot_set()
            } else {
                graph.select(&args.names)?
            };
            let _lock = lock()?;
            graph::build(&graph, &targets, &ctx, &options)?;
            Ok(ExitCode::SUCCESS)
        }
        "status" => {
            let (graph, ctx) = load(args.verbose)?;
            let targets = if args.names.is_empty() {
                graph.all()
            } else {
                graph.select(&args.names)?
            };
            graph::status(&graph, &targets, &ctx)?;
            Ok(ExitCode::SUCCESS)
        }
        "clean" => {
            let (graph, _) = load(args.verbose)?;
            let _lock = lock()?;
            if args.names.is_empty() {
                nodes::detach_derived()?;
                for dir in [aim_paths::out(), aim_paths::cache()] {
                    aim_android_image::assemble::force_remove(&dir)
                        .map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                println!("removed target/aim and target/aim-cache");
            } else {
                for name in graph.select(&args.names)? {
                    nodes::clean(graph.node(&name))?;
                    println!("  clean  {name}");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "test" => {
            let (graph, ctx) = load(args.verbose)?;
            let _lock = lock()?;
            test::run(&graph, &ctx, &options, args.integration, args.timeout)
        }
        "boot" => {
            let (graph, ctx) = load(args.verbose)?;
            {
                let _lock = lock()?;
                graph::build(&graph, &graph.boot_set(), &ctx, &options)?;
            }
            boot::run(&ctx, args.data.as_deref(), args.windows, &args.rest)
        }
        "bench" if !args.compare.is_empty() => bench::compare(&args.compare[0], &args.compare[1]),
        "bench" => {
            let (graph, ctx) = load(args.verbose)?;
            {
                let _lock = lock()?;
                graph::build(&graph, &graph.boot_set(), &ctx, &options)?;
            }
            let options = bench::Options {
                runs: args.runs,
                keep: args.keep,
            };
            bench::run(&ctx, &options)
        }
        "storage" => {
            storage::run(&args.names)?;
            Ok(ExitCode::SUCCESS)
        }
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}

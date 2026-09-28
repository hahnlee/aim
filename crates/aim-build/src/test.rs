//! `cargo aim test`: builds what the tests need, then runs every test
//! binary of the host crates (release, as the integration tests run
//! linux-run) on its own, with a timeout that kills its process group.
//!
//! Unit tests (lib and bin targets) always run; `--integration` adds the
//! `tests/` targets and builds every node first. A test that skipped for a
//! missing input (`aim_paths::skip`) is an error here.

use crate::cargo;
use crate::graph::{self, Ctx, Graph, Options};
use crate::log::Log;
use serde_json::Value;
use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

/// Test targets `cargo aim test` does not run: (package, target, why).
const EXCLUDED: &[(&str, &str, &str)] =
    &[("aim-linux-abi", "threads", "#219: hangs intermittently")];

struct TestBinary {
    package: String,
    target: String,
    kind: String,
    executable: PathBuf,
    dir: PathBuf,
}

fn compile(ctx: &Ctx) -> Result<Vec<TestBinary>, String> {
    let mut cmd = cargo::cargo();
    cmd.current_dir(aim_paths::root()).args([
        "test",
        "--release",
        "--locked",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]);
    if !ctx.verbose {
        cmd.arg("--quiet");
    }
    let out = cmd
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo test: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo test --no-run: {}", out.status));
    }
    let mut binaries = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" || message["profile"]["test"] != true {
            continue;
        }
        let Some(executable) = message["executable"].as_str() else {
            continue;
        };
        let manifest = message["manifest_path"].as_str().unwrap_or_default();
        binaries.push(TestBinary {
            package: ctx
                .workspace
                .package_name(message["package_id"].as_str().unwrap_or_default()),
            target: message["target"]["name"]
                .as_str()
                .unwrap_or_default()
                .into(),
            kind: message["target"]["kind"][0]
                .as_str()
                .unwrap_or_default()
                .into(),
            executable: executable.into(),
            dir: Path::new(manifest)
                .parent()
                .unwrap_or(Path::new("."))
                .into(),
        });
    }
    binaries.sort_by(|a, b| (&a.package, &a.target).cmp(&(&b.package, &b.target)));
    Ok(binaries)
}

/// The counts of libtest's `test result:` lines.
fn summary(log: &Path) -> String {
    let text = fs::read_to_string(log).unwrap_or_default();
    let (mut passed, mut failed, mut ignored) = (0, 0, 0);
    for line in text.lines().filter(|l| l.starts_with("test result:")) {
        let count = |what: &str| {
            line.split(';')
                .find(|part| part.trim().ends_with(what))
                .and_then(|part| part.trim().split(' ').rev().nth(1))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        passed += count("passed");
        failed += count("failed");
        ignored += count("ignored");
    }
    format!("{passed} passed, {failed} failed, {ignored} ignored")
}

enum Outcome {
    Passed,
    Failed(String),
}

fn run_one(binary: &TestBinary, log_path: &Path, timeout: Duration) -> Result<Outcome, String> {
    fs::create_dir_all(log_path.parent().unwrap()).map_err(|e| e.to_string())?;
    let log = File::create(log_path).map_err(|e| e.to_string())?;
    let mut child = Command::new(&binary.executable)
        .current_dir(&binary.dir)
        .env("CARGO_MANIFEST_DIR", &binary.dir)
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{}: {e}", binary.executable.display()))?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            // Whatever the tests left behind (a guest's linux-run) goes too.
            // SAFETY: signals the child's own process group.
            unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
            return Ok(if status.success() {
                Outcome::Passed
            } else {
                Outcome::Failed(status.to_string())
            });
        }
        if Instant::now() > deadline {
            // SAFETY: as above.
            unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
            let _ = child.wait();
            return Ok(Outcome::Failed(format!(
                "timed out after {} s",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn run(
    graph: &Graph,
    ctx: &Ctx,
    options: &Options,
    integration: bool,
    timeout: Duration,
) -> Result<ExitCode, String> {
    if integration {
        graph::build(graph, &graph.all(), ctx, options)?;
    }
    let binaries = compile(ctx)?;
    let skips = aim_paths::test_skips();
    let _ = fs::remove_dir_all(&skips);
    let started = Instant::now();
    let mut failures = Vec::new();
    let mut ran = 0;
    for binary in &binaries {
        let name = format!("{}/{}", binary.package, binary.target);
        let unit = matches!(binary.kind.as_str(), "lib" | "bin" | "proc-macro");
        if !unit && (!integration || binary.kind != "test") {
            continue;
        }
        if let Some((_, _, why)) = EXCLUDED
            .iter()
            .find(|(p, t, _)| *p == binary.package && *t == binary.target)
        {
            if integration {
                println!("  excluded {name} ({why})");
            }
            continue;
        }
        let log_path = aim_paths::cache().join("logs/test").join(format!(
            "{}~{}~{}.log",
            binary.package, binary.kind, binary.target
        ));
        let begun = Instant::now();
        let outcome = run_one(binary, &log_path, timeout)?;
        ran += 1;
        let took = begun.elapsed().as_secs_f64();
        match outcome {
            Outcome::Passed => println!("  ok     {name} ({}; {took:.1} s)", summary(&log_path)),
            Outcome::Failed(why) => {
                println!(
                    "  FAILED {name}: {why} ({}; {took:.1} s)",
                    summary(&log_path)
                );
                println!("{}", Log::tail(&log_path, 30));
                println!(
                    "  (full log: {})",
                    crate::stamp::display(&log_path).display()
                );
                failures.push(format!("{name}: {why}"));
            }
        }
    }
    let mut skipped: Vec<String> = fs::read_dir(&skips)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .map(|s| s.trim().to_string())
        .collect();
    skipped.sort();
    for skip in &skipped {
        println!("  SKIPPED (an error under cargo aim test) {skip}");
    }
    println!(
        "{ran} test binaries, {} failed, {} skipped tests in {:.1} s",
        failures.len(),
        skipped.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(if failures.is_empty() && skipped.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

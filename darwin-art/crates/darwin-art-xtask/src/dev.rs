use std::collections::BTreeSet;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::graph::inputs::graph_inputs;

#[derive(Debug, Default, Eq, PartialEq)]
struct ChangePlan {
    rust: bool,
    support: bool,
    native: bool,
    foundation: bool,
    audits: BTreeSet<PathBuf>,
}

pub(crate) fn run(arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let root = repository_root()?;
    let mut arguments = arguments.peekable();
    match arguments.next().as_deref() {
        Some("check") => check(&root, arguments.collect()),
        Some("build") => build(&root, false),
        Some("status") => status(&root),
        Some("explain") => explain(&root, arguments.map(PathBuf::from).collect()),
        Some("providers") => providers(&root),
        Some("full") => full(&root),
        _ => Err(
            "usage: cargo xtask <check [--full] [PATH ...] | build | status | explain PATH ... | providers | full>"
                .to_owned(),
        ),
    }
}

fn repository_root() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| "xtask manifest is not beneath the repository root".to_owned())
}

fn changed_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = BTreeSet::new();
    for arguments in [
        ["diff", "--name-only", "HEAD"].as_slice(),
        ["ls-files", "--others", "--exclude-standard"].as_slice(),
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(root)
            .output()
            .map_err(|error| format!("failed to launch git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if !line.is_empty() {
                paths.insert(PathBuf::from(line));
            }
        }
    }
    Ok(paths.into_iter().collect())
}

fn classify(root: &Path, paths: &[PathBuf]) -> ChangePlan {
    let native_inputs = graph_inputs(root).into_iter().collect::<BTreeSet<_>>();
    let mut plan = ChangePlan::default();
    for path in paths {
        let text = path.to_string_lossy();
        let file_name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        plan.support |= text.starts_with("runtime/framework/") || text == "tools/dex-inspect.cc";
        plan.rust |= path.extension().is_some_and(|extension| extension == "rs")
            || matches!(file_name, "Cargo.toml" | "Cargo.lock" | "config.toml");
        let foundation = text.starts_with("patches/")
            || text.starts_with("upstream/")
            || text.starts_with("_aosp/")
            || text == "sources.lock"
            || text == "bootclasspath.lock";
        plan.foundation |= foundation;
        let native_source = matches!(
            path.extension().and_then(OsStr::to_str),
            Some("c" | "cc" | "cpp" | "cxx" | "m" | "mm" | "S" | "h" | "hpp" | "inc")
        );
        plan.native |= native_inputs.contains(path)
            || foundation
            || (native_source
                && (text.starts_with("compat/")
                    || text.starts_with("probes/")
                    || text.starts_with("tools/")));

        let provider_implementation = matches!(
            path.components()
                .nth(2)
                .and_then(|component| component.as_os_str().to_str()),
            Some("src" | "include" | "generated" | "probes")
        ) || file_name == "build.rs";
        if provider_implementation
            && let Some(tool) = path
                .components()
                .nth(1)
                .and_then(|component| component.as_os_str().to_str())
            && text.starts_with("tools/")
        {
            let audit = PathBuf::from("tools").join(tool).join("audit.sh");
            if root.join(&audit).is_file() {
                plan.audits.insert(audit);
                // Provider crates are linked into the final runtime even when
                // the changed implementation is Rust rather than a native TU.
                plan.native = true;
            }
        }
    }
    plan
}

fn check(root: &Path, mut arguments: Vec<String>) -> Result<(), String> {
    let full = take_flag(&mut arguments, "--full");
    let paths = if arguments.is_empty() {
        changed_paths(root)?
    } else {
        arguments.into_iter().map(PathBuf::from).collect()
    };
    let mut plan = classify(root, &paths);
    if paths.is_empty() {
        plan.rust = true;
    }
    print_plan(&paths, &plan);
    run_command(root, "format", cargo(), ["fmt", "--all", "--", "--check"])?;
    if plan.rust || full {
        run_command(
            root,
            "Rust workspace",
            cargo(),
            ["check", "--workspace", "--all-targets"],
        )?;
    }
    for audit in &plan.audits {
        run_command(
            root,
            &audit.display().to_string(),
            "bash",
            [audit.as_os_str()],
        )?;
    }
    if plan.native || full {
        build(root, full)?;
    } else if plan.support {
        run_command(
            root,
            "production support DEX",
            cargo(),
            [
                "run",
                "-q",
                "-p",
                "art-bootstrap",
                "--",
                "build-runtime-support-dex-incremental",
            ],
        )?;
    }
    Ok(())
}

fn build(root: &Path, full: bool) -> Result<(), String> {
    let command = if full {
        "audit-runtime-graphics-link"
    } else {
        "audit-runtime-graphics-link-incremental"
    };
    run_command(
        root,
        if full {
            "full native graph"
        } else {
            "incremental native graph"
        },
        cargo(),
        ["run", "-q", "-p", "art-bootstrap", "--", command],
    )?;
    run_command(
        root,
        "production runtime payload",
        cargo(),
        [
            "run",
            "-q",
            "-p",
            "art-bootstrap",
            "--",
            "build-runtime-payload-incremental",
        ],
    )?;
    run_command(
        root,
        "immutable Android system image",
        "bash",
        ["tools/build-android-system-image.sh"],
    )
}

fn full(root: &Path) -> Result<(), String> {
    run_command(root, "format", cargo(), ["fmt", "--all", "--", "--check"])?;
    run_command(
        root,
        "Rust workspace tests",
        cargo(),
        ["test", "--workspace", "--all-targets"],
    )?;
    providers(root)?;
    build(root, true)
}

fn provider_audits(root: &Path) -> Result<Vec<PathBuf>, String> {
    let tools = root.join("tools");
    let mut audits = Vec::new();
    for entry in fs::read_dir(&tools).map_err(|error| format!("read tools/: {error}"))? {
        let entry = entry.map_err(|error| format!("read tools/ entry: {error}"))?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with("bionic-") {
            let audit = entry.path().join("audit.sh");
            if audit.is_file() {
                audits.push(audit.strip_prefix(root).unwrap_or(&audit).to_path_buf());
            }
        }
    }
    audits.sort();
    Ok(audits)
}

fn providers(root: &Path) -> Result<(), String> {
    let audits = provider_audits(root)?;
    if audits.is_empty() {
        return Err("no Bionic provider audits found".to_owned());
    }
    for (index, audit) in audits.iter().enumerate() {
        run_command(
            root,
            &format!(
                "provider {}/{}: {}",
                index + 1,
                audits.len(),
                audit.display()
            ),
            "bash",
            [audit.as_os_str()],
        )?;
    }
    println!("\nAll {} Bionic provider audits passed.", audits.len());
    Ok(())
}

fn explain(root: &Path, paths: Vec<PathBuf>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("cargo xtask explain requires at least one repository path".to_owned());
    }
    for path in paths {
        let plan = classify(root, std::slice::from_ref(&path));
        println!("{}", path.display());
        println!("  Rust workspace: {}", yes_no(plan.rust));
        println!("  Native graph: {}", yes_no(plan.native));
        println!("  Foundation rebuild: {}", yes_no(plan.foundation));
        if plan.audits.is_empty() {
            println!("  Provider audit: none");
        } else {
            for audit in plan.audits {
                println!("  Provider audit: {}", audit.display());
            }
        }
    }
    Ok(())
}

fn status(root: &Path) -> Result<(), String> {
    let package_count = fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|error| format!("read Cargo.toml: {error}"))?
        .lines()
        .filter(|line| line.trim_start().starts_with('"'))
        .count();
    println!("Darwin ART build layers");
    println!("  Rust workspace     {package_count} packages, one Cargo.lock and target/");
    print_artifact(
        root,
        "AOSP runtime",
        "_build/runtime-graphics-bootstrap/libart-runtime-graphics-bootstrap-darwin.a",
    );
    print_artifact(
        root,
        "provider closure",
        "_build/bionic-runtime-provider-closure/libdarwin-art-bionic-rust-providers.a",
    );
    print_artifact(
        root,
        "app runtime",
        "_build/runtime-graphics-link-probe/libdarwin_art_runtime_graphics.dylib",
    );
    println!();
    println!("  check changed files  cargo xtask check");
    println!("  incremental build    cargo xtask build");
    println!("  release gate         cargo xtask full");
    println!("  provider gate        cargo xtask providers");
    println!("  explain a path       cargo xtask explain PATH");
    Ok(())
}

fn print_artifact(root: &Path, label: &str, relative: &str) {
    match fs::metadata(root.join(relative)) {
        Ok(metadata) => println!(
            "  {label:<18} cached {} MiB",
            metadata.len() / (1024 * 1024)
        ),
        Err(_) => println!("  {label:<18} missing"),
    }
}

fn print_plan(paths: &[PathBuf], plan: &ChangePlan) {
    println!("Darwin ART affected plan: {} path(s)", paths.len());
    println!("  Rust workspace: {}", yes_no(plan.rust));
    println!("  Native graph: {}", yes_no(plan.native));
    println!("  Foundation rebuild: {}", yes_no(plan.foundation));
    for audit in &plan.audits {
        println!("  Provider audit: {}", audit.display());
    }
}

fn run_command<I, S>(
    root: &Path,
    label: &str,
    program: impl AsRef<OsStr>,
    arguments: I,
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    println!("\n[{label}]");
    let status = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .status()
        .map_err(|error| format!("failed to launch {label}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{label} failed with {status}"))
    }
}

fn cargo() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn take_flag(arguments: &mut Vec<String>, flag: &str) -> bool {
    let Some(index) = arguments.iter().position(|argument| argument == flag) else {
        return false;
    };
    arguments.remove(index);
    true
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_separates_rust_provider_and_foundation_changes() {
        let root = repository_root().unwrap();
        let rust = classify(
            &root,
            &[PathBuf::from("crates/darwin-art-host/src/main.rs")],
        );
        assert!(rust.rust);
        assert!(!rust.native);
        let java = classify(
            &root,
            &[PathBuf::from("runtime/framework/support-sources.txt")],
        );
        assert!(java.support);
        assert!(!java.native);

        let provider = classify(&root, &[PathBuf::from("tools/bionic-vm-facade/src/host.c")]);
        assert!(provider.native);
        assert!(
            provider
                .audits
                .contains(Path::new("tools/bionic-vm-facade/audit.sh"))
        );

        let foundation = classify(&root, &[PathBuf::from("patches/art/0001-example.patch")]);
        assert!(foundation.native);
        assert!(foundation.foundation);
    }

    #[test]
    fn provider_gate_discovers_audits_in_stable_order() {
        let root = repository_root().unwrap();
        let audits = provider_audits(&root).unwrap();
        assert_eq!(audits.len(), 35);
        assert!(audits.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(audits.contains(&PathBuf::from("tools/bionic-provider-namespace/audit.sh")));
    }
}

//! `android-image`: derive the system image from the original Android image.

use aim_android_image::diff::{self, Header};
use aim_android_image::{Outcome, assemble, identity, load};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  android-image assemble --original ORIGDIR --manifest MANIFEST --out OUTDIR [options]
  android-image identity --original ORIGDIR --manifest MANIFEST [options]
  android-image diff     --original ORIGDIR --manifest MANIFEST [options]

options:
  --original-identity HEX  the original's sha256 identity; defaults to ORIGDIR.identity,
                           which android-image-extract records beside the tree it
                           extracts, or ORIGDIR/.identity (those given must agree)
  --source-root DIR        root that manifest sources are relative to; defaults to the
                           directory above the manifest's (image/overlay.toml -> .)

assemble clones ORIGDIR (APFS clonefile, so OUTDIR must be on the same volume), applies
the overlay and publishes a read-only tree at OUTDIR whose .identity names it. An OUTDIR
that already holds the same identity is reused untouched.
identity prints the derived identity without building.
diff prints what the derived image adds, replaces and removes, with sizes and reasons.";

struct Options {
    command: String,
    original: PathBuf,
    manifest: PathBuf,
    out: Option<PathBuf>,
    original_identity: Option<String>,
    source_root: Option<PathBuf>,
}

fn parse_args(args: Vec<String>) -> Result<Options, String> {
    let mut args = args.into_iter();
    let command = args.next().ok_or("missing command")?;
    if !["assemble", "identity", "diff"].contains(&command.as_str()) {
        return Err(format!("unknown command `{command}`"));
    }
    let (mut original, mut manifest, mut out, mut original_identity, mut source_root) =
        (None, None, None, None, None);
    while let Some(flag) = args.next() {
        let slot = match flag.as_str() {
            "--original" => &mut original,
            "--manifest" => &mut manifest,
            "--out" if command == "assemble" => &mut out,
            "--original-identity" => &mut original_identity,
            "--source-root" => &mut source_root,
            _ => return Err(format!("unexpected argument `{flag}`")),
        };
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        if slot.replace(value).is_some() {
            return Err(format!("{flag} given twice"));
        }
    }
    let original = original.ok_or("--original is required")?;
    let manifest = manifest.ok_or("--manifest is required")?;
    if command == "assemble" && out.is_none() {
        return Err("--out is required".into());
    }
    Ok(Options {
        command,
        original: original.into(),
        manifest: manifest.into(),
        out: out.map(PathBuf::from),
        original_identity,
        source_root: source_root.map(PathBuf::from),
    })
}

fn default_source_root(manifest: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(manifest)
        .map_err(|error| format!("{}: {error}", manifest.display()))?;
    absolute
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "{}: cannot infer the source root; pass --source-root",
                manifest.display()
            )
        })
}

fn run(options: Options) -> Result<(), String> {
    let source_root = match &options.source_root {
        Some(root) => root.clone(),
        None => default_source_root(&options.manifest)?,
    };
    let original_identity =
        identity::original_identity(&options.original, options.original_identity.as_deref())?;
    let (_, plan) =
        load(&options.manifest, &options.original, &source_root).map_err(|problems| {
            let mut message = format!("{}: invalid overlay manifest", options.manifest.display());
            for problem in problems {
                message.push_str(&format!("\n  {problem}"));
            }
            message
        })?;
    let derived = original_identity
        .as_deref()
        .map(|original| identity::compute(original, &plan));

    match options.command.as_str() {
        "diff" => {
            print!(
                "{}",
                diff::render(
                    &Header {
                        manifest: &options.manifest.display().to_string(),
                        original: &options.original.display().to_string(),
                        original_identity: original_identity.as_deref(),
                        derived_identity: derived.as_ref().map(|d| d.hex.as_str()),
                    },
                    &plan,
                )
            );
            Ok(())
        }
        command => {
            let derived = derived.ok_or_else(|| {
                format!(
                    "the original's identity is unknown: neither {} nor {} exists; pass --original-identity",
                    identity::extraction_identity_path(&options.original).display(),
                    options.original.join(".identity").display()
                )
            })?;
            if command == "identity" {
                println!("{}", derived.hex);
                return Ok(());
            }
            let out = options.out.as_deref().expect("checked by parse_args");
            let outcome = assemble(&plan, &options.original, &derived, out)?;
            let verb = match outcome {
                Outcome::Built => "built",
                Outcome::Reused => "reused",
            };
            println!("{verb} {} {}", derived.hex, out.display());
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = match parse_args(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("android-image: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("android-image: {error}");
            ExitCode::FAILURE
        }
    }
}

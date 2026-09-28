//! The deviation report: everything the derived image adds to, replaces in
//! and removes from the original, with sizes and reasons.

use crate::manifest::Kind;
use crate::plan::{Footprint, Plan};
use std::fmt::Write as _;

pub struct Header<'a> {
    pub manifest: &'a str,
    pub original: &'a str,
    pub original_identity: Option<&'a str>,
    pub derived_identity: Option<&'a str>,
}

pub fn render(header: &Header<'_>, plan: &Plan) -> String {
    let mut out = String::new();
    let unknown = "unknown (no ORIGDIR.identity; pass --original-identity)";
    writeln!(out, "original  {}", header.original).unwrap();
    writeln!(
        out,
        "          identity {}",
        header.original_identity.unwrap_or(unknown)
    )
    .unwrap();
    writeln!(out, "manifest  {}", header.manifest).unwrap();
    writeln!(
        out,
        "derived   identity {}",
        header.derived_identity.unwrap_or(unknown)
    )
    .unwrap();
    writeln!(out).unwrap();

    if plan.steps.is_empty() {
        writeln!(
            out,
            "no deviations: the derived image is the original plus its identity receipt"
        )
        .unwrap();
        return out;
    }

    let width = plan
        .steps
        .iter()
        .map(|step| step.path.len())
        .max()
        .unwrap_or(0);
    let (mut added, mut added_bytes) = (0u64, 0u64);
    let (mut replaced, mut replaced_delta) = (0u64, 0i128);
    let (mut removed, mut removed_bytes) = (0u64, 0u64);
    for step in &plan.steps {
        let (sign, size) = match step.kind {
            Kind::Add => {
                let bytes = step.source.as_ref().map_or(0, |source| source.bytes);
                added += 1;
                added_bytes += bytes;
                ('+', human(bytes))
            }
            Kind::Replace => {
                let new = step.source.as_ref().map_or(0, |source| source.bytes);
                let old = step.original.map_or(0, Footprint::bytes);
                let delta = i128::from(new) - i128::from(old);
                replaced += 1;
                replaced_delta += delta;
                (
                    '~',
                    format!("{} -> {} ({})", human(old), human(new), signed(delta)),
                )
            }
            Kind::Remove => {
                let footprint = step.original.unwrap_or(Footprint::Other);
                removed += 1;
                removed_bytes += footprint.bytes();
                ('-', describe(footprint))
            }
        };
        writeln!(
            out,
            "{sign} {:<7}  {:<width$}  {size}",
            step.kind.as_str(),
            step.path
        )
        .unwrap();
        if let Some(source) = &step.source {
            writeln!(
                out,
                "             from   {} (sha256 {})",
                source.declared, source.sha256
            )
            .unwrap();
        }
        if let Some(reason) = &step.reason {
            for (index, line) in reason.lines().enumerate() {
                let label = if index == 0 { "reason" } else { "      " };
                writeln!(out, "             {label} {line}").unwrap();
            }
        }
    }
    writeln!(out).unwrap();
    writeln!(
        out,
        "{added} added ({}), {replaced} replaced ({}), {removed} removed ({})",
        signed(i128::from(added_bytes)),
        signed(replaced_delta),
        signed(-i128::from(removed_bytes)),
    )
    .unwrap();
    out
}

fn describe(footprint: Footprint) -> String {
    match footprint {
        Footprint::File { bytes } => human(bytes),
        Footprint::Symlink => "symlink".into(),
        Footprint::Directory { files, bytes } => {
            let noun = if files == 1 { "file" } else { "files" };
            format!("directory, {files} {noun}, {}", human(bytes))
        }
        Footprint::Other => "special file".into(),
    }
}

fn signed(delta: i128) -> String {
    let sign = if delta < 0 { '-' } else { '+' };
    format!("{sign}{}", human(delta.unsigned_abs() as u64))
}

/// Bytes below 1 KiB exactly, larger sizes in binary units to one decimal.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

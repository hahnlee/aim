//! Whole-archive extraction into the device's mounted view:
//!
//! - `OUTDIR/` is the system partition root (system-as-root);
//! - every other dynamic or GPT partition is written into its mount point,
//!   `OUTDIR/<partition>/` (system_ext, product, vendor, system_dlkm, odm...);
//! - the ramdisk (first-stage root) goes to `OUTDIR/ramdisk/`;
//! - every APEX/CAPEX payload found where apexd scans goes to
//!   `OUTDIR/apex/<manifest name>/`, the highest version winning.
//!
//! Disk images are inflated to one temporary file beside OUTDIR, which is
//! removed as soon as that image is done.
use crate::cpio;
use crate::erofs::{self, Erofs};
use crate::ext4::{self, Ext4};
use crate::source::{Extent, ExtentView, FileSource, ReadAt, Reader};
use crate::tree::{Materializer, Report, Tree};
use crate::zip::Archive;
use crate::{Result, apex, gpt, invalid, le32, lp, lz4};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

const DISK_IMAGES: &[&str] = &["system.img", "vendor.img"];
const RAMDISK: &str = "ramdisk.img";
const APEX_DIRS: &[&str] = &[
    "system/apex",
    "system_ext/apex",
    "product/apex",
    "vendor/apex",
    "odm/apex",
];
const MAX_RAMDISK: usize = 512 * 1024 * 1024;

/// Removes the inflated image when dropped, including on errors.
struct Temporary(PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// One line per extracted unit, for the final summary.
pub struct Line {
    pub unit: String,
    pub detail: String,
    pub report: Report,
}

fn entry_named<'a>(archive: &'a Archive<'_>, file: &str) -> Option<&'a crate::zip::Entry> {
    archive.entries.iter().find(|entry| {
        entry.name == file.as_bytes() || entry.name.ends_with(format!("/{file}").as_bytes())
    })
}

/// Named partitions, each as absolute extents of its disk image.
type Partitions = Vec<(String, Vec<Extent>)>;

/// The disk layout's description and its logical partitions.
fn partitions(disk: &dyn ReadAt) -> Result<(String, Partitions)> {
    let whole = |disk: &dyn ReadAt| {
        vec![Extent {
            logical: 0,
            physical: Some(0),
            length: disk.size(),
        }]
    };
    if !gpt::is_gpt(disk) {
        return Ok(("raw".into(), vec![(String::new(), whole(disk))]));
    }
    let table = gpt::read(disk)?;
    let mut out = Vec::new();
    let mut layout = format!("GPT {}.{}", table.version.0, table.version.1);
    for partition in &table.partitions {
        if partition.name != "super" {
            out.push((
                partition.name.clone(),
                vec![Extent {
                    logical: 0,
                    physical: Some(partition.offset),
                    length: partition.size,
                }],
            ));
            continue;
        }
        let region = crate::source::Slice::new(disk, partition.offset, partition.size)?;
        let metadata = lp::read(&region)?;
        layout += &format!(
            " + super (LP {}.{}, {} partitions)",
            metadata.version.0,
            metadata.version.1,
            metadata.partitions.len()
        );
        for lp in metadata.partitions {
            if lp.extents.is_empty() {
                continue;
            }
            let name = match lp.name.strip_suffix("_a") {
                Some(name) => name.to_owned(),
                None if lp.name.ends_with("_b") => continue,
                None => lp.name,
            };
            let extents = lp
                .extents
                .into_iter()
                .map(|e| Extent {
                    physical: e.physical.map(|p| p + partition.offset),
                    ..e
                })
                .collect();
            out.push((name, extents));
        }
    }
    Ok((layout, out))
}

fn filesystem<T>(
    device: &dyn ReadAt,
    visit: impl FnOnce(&dyn Tree, String) -> Result<T>,
) -> Result<Option<T>> {
    if erofs::is_erofs(device) {
        let fs = Erofs::open(device)?;
        let detail = format!("EROFS blocksize={} incompat={:#x}", fs.block, fs.incompat);
        visit(&fs, detail).map(Some)
    } else if ext4::is_ext4(device) {
        let fs = Ext4::open(device)?;
        let detail = format!("ext4 blocksize={}", fs.block);
        visit(&fs, detail).map(Some)
    } else {
        Ok(None)
    }
}

/// SHA-256 of the bytes a filesystem declares as its own (its superblock
/// block count), so copies padded to different partition sizes compare equal.
fn filesystem_digest(device: &dyn ReadAt) -> Result<[u8; 32]> {
    let sb = device.read_vec(1024, 1024)?;
    let length = if erofs::is_erofs(device) {
        u64::from(le32(&sb, 36)?) << sb[12]
    } else if ext4::is_ext4(device) {
        let high = if le32(&sb, 0x60)? & 0x80 != 0 {
            le32(&sb, 0x150)?
        } else {
            0
        };
        (u64::from(le32(&sb, 4)?) | u64::from(high) << 32) * (1024 << le32(&sb, 0x18)?)
    } else {
        return Err(invalid("not a filesystem"));
    };
    let mut hasher = Sha256::new();
    std::io::copy(&mut Reader::new(device, 0, length)?, &mut hasher)?;
    Ok(hasher.finalize().into())
}

fn mount_point(out: &Path, partition: &str) -> PathBuf {
    if partition == "system" {
        out.to_owned()
    } else {
        out.join(partition)
    }
}

pub fn extract(archive_path: &Path, out: &Path) -> Result<Vec<Line>> {
    if fs::symlink_metadata(out).is_ok() {
        return Err(invalid(format!("{} already exists", out.display())));
    }
    // A bare OUTDIR name has the empty path as its parent: the current directory.
    let parent = out
        .parent()
        .map(|p| {
            if p.as_os_str().is_empty() {
                Path::new(".")
            } else {
                p
            }
        })
        .filter(|p| p.is_dir())
        .ok_or_else(|| invalid("OUTDIR's parent directory must exist"))?;
    let name = out
        .file_name()
        .ok_or_else(|| invalid("OUTDIR has no final component"))?
        .to_string_lossy()
        .into_owned();
    let source = FileSource::open(archive_path)?;
    let archive = Archive::open(&source)?;
    let mut materializer = Materializer::default();
    let mut lines = Vec::new();
    // Partition name -> (source image, filesystem digest).
    let mut done: BTreeMap<String, (String, [u8; 32])> = BTreeMap::new();

    for image in DISK_IMAGES {
        let Some(entry) = entry_named(&archive, image) else {
            continue;
        };
        let temporary = Temporary(parent.join(format!(".{name}.{image}.partial")));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary.0)
            .map_err(|e| invalid(format!("create {}: {e}", temporary.0.display())))?;
        let mut writer = BufWriter::with_capacity(4 << 20, file);
        archive.copy_to(entry, &mut writer)?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        let disk = FileSource::open(&temporary.0)?;
        let (layout, list) = partitions(&disk)?;
        // The system root must exist before other partitions mount into it.
        let mut ordered = list;
        ordered.sort_by_key(|(name, _)| name != "system");
        for (partition, extents) in ordered {
            let partition = if partition.is_empty() {
                image.trim_end_matches(".img").to_owned()
            } else {
                partition
            };
            let unit = format!("{image}:{partition}");
            let view = ExtentView::new(&disk, extents)?;
            if let Some((first, digest)) = done.get(&partition) {
                let same = filesystem_digest(&view)? == *digest;
                if !same {
                    return Err(invalid(format!(
                        "{image} holds a {partition} filesystem that differs from {first}'s"
                    )));
                }
                lines.push(Line {
                    unit,
                    detail: format!("skipped: filesystem identical to {first}:{partition}"),
                    report: Report::default(),
                });
                continue;
            }
            let size = view.size();
            let destination = mount_point(out, &partition);
            let extracted = filesystem(&view, |tree, detail| {
                Ok((detail, materializer.extract(tree, &destination)?))
            })?;
            match extracted {
                Some((detail, report)) => {
                    done.insert(
                        partition.clone(),
                        (image.to_string(), filesystem_digest(&view)?),
                    );
                    lines.push(Line {
                        unit,
                        detail: format!("{layout}; {detail}; {size} bytes"),
                        report,
                    });
                }
                None => lines.push(Line {
                    unit,
                    detail: format!("skipped: no filesystem ({size} bytes)"),
                    report: Report::default(),
                }),
            }
        }
        drop(disk);
        drop(temporary);
    }

    if let Some(entry) = entry_named(&archive, RAMDISK) {
        let raw = archive.read(entry, MAX_RAMDISK as u64)?;
        let (format, bytes) = if raw.starts_with(&lz4::LEGACY_MAGIC.to_le_bytes()) {
            ("legacy LZ4", lz4::decode_legacy(&raw, MAX_RAMDISK)?)
        } else if raw.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            flate2::read::MultiGzDecoder::new(&raw[..])
                .take(MAX_RAMDISK as u64)
                .read_to_end(&mut out)?;
            ("gzip", out)
        } else {
            ("uncompressed", raw)
        };
        let tree = cpio::Archive::parse(&bytes)?;
        let report = materializer.extract(&tree, &out.join("ramdisk"))?;
        lines.push(Line {
            unit: RAMDISK.into(),
            detail: format!("{format} cpio (newc); {} bytes", bytes.len()),
            report,
        });
    }

    lines.extend(extract_apexes(&mut materializer, out)?);
    let total = materializer.finish()?;
    lines.push(Line {
        unit: "total".into(),
        detail: String::new(),
        report: total,
    });
    Ok(lines)
}

/// Records the original's identity, the archive's sha256, in
/// `OUTDIR.identity` beside the extracted tree (the tree itself stays exactly
/// the archive's content). `android-image` reads it as the original's
/// identity. Returns the identity and the file.
pub fn record_identity(archive: &Path, out: &Path) -> Result<(String, PathBuf)> {
    if !out.is_dir() {
        return Err(invalid(format!(
            "{} is not an extracted tree",
            out.display()
        )));
    }
    let mut hasher = Sha256::new();
    std::io::copy(&mut File::open(archive)?, &mut hasher)?;
    let identity: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut name = out
        .file_name()
        .ok_or_else(|| invalid("OUTDIR has no final component"))?
        .to_os_string();
    name.push(".identity");
    let path = out.with_file_name(name);
    let mut partial = path.clone().into_os_string();
    partial.push(".partial");
    fs::write(&partial, format!("{identity}\n"))?;
    fs::rename(&partial, &path)?;
    Ok((identity, path))
}

struct Candidate {
    path: PathBuf,
    compressed: bool,
    version: u64,
}

fn open_apex(candidate_path: &Path, compressed: bool) -> Result<Vec<u8>> {
    let source = FileSource::open(candidate_path)?;
    if compressed {
        apex::original_apex(&source)
    } else {
        let mut bytes = Vec::with_capacity(source.size() as usize);
        File::open(candidate_path)?.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

fn extract_apexes(materializer: &mut Materializer, out: &Path) -> Result<Vec<Line>> {
    let mut lines = Vec::new();
    let mut chosen: BTreeMap<String, Candidate> = BTreeMap::new();
    for directory in APEX_DIRS {
        let Ok(entries) = fs::read_dir(out.join(directory)) else {
            continue;
        };
        let mut files: Vec<_> = entries.collect::<std::io::Result<_>>()?;
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let compressed = file_name.ends_with(".capex");
            if !(compressed || file_name.ends_with(".apex")) || !entry.file_type()?.is_file() {
                continue;
            }
            let bytes = open_apex(&path, compressed)
                .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
            let manifest = apex::with_payload(&bytes, |manifest, _, _| Ok(manifest.clone()))
                .map_err(|e| invalid(format!("{}: {e}", path.display())))?;
            let relative = format!("{directory}/{file_name}");
            match chosen.get(&manifest.name) {
                Some(existing) if existing.version >= manifest.version => {
                    lines.push(Line {
                        unit: format!("apex:{}", manifest.name),
                        detail: format!(
                            "skipped /{relative} v{}: /{} v{} is preferred",
                            manifest.version,
                            existing
                                .path
                                .strip_prefix(out)
                                .unwrap_or(&existing.path)
                                .display(),
                            existing.version
                        ),
                        report: Report::default(),
                    });
                }
                _ => {
                    chosen.insert(
                        manifest.name,
                        Candidate {
                            path,
                            compressed,
                            version: manifest.version,
                        },
                    );
                }
            }
        }
    }
    let root = out.join("apex");
    if fs::symlink_metadata(&root).is_err() {
        fs::create_dir(&root)?;
    }
    for (name, candidate) in chosen {
        let bytes = open_apex(&candidate.path, candidate.compressed)?;
        let destination = root.join(&name);
        let (detail, report) = apex::with_payload(&bytes, |manifest, tree, format| {
            let report = materializer.extract(tree, &destination)?;
            Ok((
                format!(
                    "/{} v{} ({}, {format} payload)",
                    candidate
                        .path
                        .strip_prefix(out)
                        .unwrap_or(&candidate.path)
                        .display(),
                    manifest.version,
                    if candidate.compressed {
                        "CAPEX"
                    } else {
                        "APEX"
                    }
                ),
                report,
            ))
        })
        .map_err(|e| invalid(format!("{}: {e}", candidate.path.display())))?;
        lines.push(Line {
            unit: format!("apex:{name}"),
            detail,
            report,
        });
    }
    Ok(lines)
}

/// Print the extraction summary.
pub fn print(lines: &[Line], out: &mut dyn Write) -> std::io::Result<()> {
    for line in lines {
        let r = &line.report;
        writeln!(
            out,
            "{:<48} dirs={} files={} hardlinks={} symlinks={} bytes={} special={}  {}",
            line.unit,
            r.directories,
            r.files,
            r.hardlinks,
            r.symlinks,
            r.bytes,
            r.special.len(),
            line.detail
        )?;
    }
    if let Some(total) = lines.last() {
        for (path, mode) in &total.report.special {
            writeln!(out, "special (not created): {:o} {}", mode, path.display())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_recorded_beside_the_tree() {
        let dir = crate::tree::tests::temp_dir("identity");
        fs::create_dir(&dir).unwrap();
        let archive = dir.join("image.zip");
        fs::write(&archive, b"abc").unwrap();
        let out = dir.join("tree");
        assert!(record_identity(&archive, &out).is_err());
        fs::create_dir(&out).unwrap();
        let (identity, path) = record_identity(&archive, &out).unwrap();
        // sha256("abc")
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(identity, abc);
        assert_eq!(path, dir.join("tree.identity"));
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{abc}\n"));
        assert_eq!(
            fs::read_dir(&out).unwrap().count(),
            0,
            "the tree is untouched"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}

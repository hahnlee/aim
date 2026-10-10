//! Executable derivation witnesses are computed from authenticated original bytes.
//! Mutable cache metadata is never an authority for rewritten instructions.
use crate::{
    cache::{self, Cache},
    errno::{self, Errno},
    sys::verified_source::VerifiedSource,
    xlate,
};
use std::{
    collections::HashMap,
    ops::Range,
    path::Path,
    sync::{Arc, LazyLock, Mutex, Weak},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationBinding {
    pub original_identity: aim_storage::fsverity::Identity,
    pub source_offset: u64,
    pub source_length: u64,
    pub original_sha256: String,
    pub translator_version: u32,
    pub ctr_el0: u32,
    pub expected_output_sha256: String,
    pub expected_sites_sha256: String,
}

/// Owned authenticated output snapshot; no mutable cache path escapes admission.
pub struct VerifiedArtifact {
    pub original: Arc<VerifiedSource>,
    pub binding: DerivationBinding,
    bytes: Arc<[u8]>,
    _witness: Arc<Witness>,
    /// Includes RODATA hash edits as well as code/stub generation.
    pub fips: Option<xlate::fips::Module>,
}
impl VerifiedArtifact {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn read_exact_at(&self, buffer: &mut [u8], offset: u64) -> Result<(), Errno> {
        let start = usize::try_from(offset).map_err(|_| errno::EIO)?;
        let end = start.checked_add(buffer.len()).ok_or(errno::EIO)?;
        buffer.copy_from_slice(self.bytes.get(start..end).ok_or(errno::EIO)?);
        Ok(())
    }
}

/// Authenticated original-site offsets and FIPS cross-segment dependencies.
/// The pager applies its existing relocation-dependent rewrite, not a Merkle
/// check of rewritten bytes against the original root.
pub struct SitesRewritePlan {
    pub original: Arc<VerifiedSource>,
    pub binding: DerivationBinding,
    pub sites: Arc<super::Sites>,
    /// Original PT_LOAD headers, including member-relative offsets/alignment.
    pub load_segments: Vec<xlate::elf::Phdr>,
    original_bytes: Arc<[u8]>,
    _witness: Arc<Witness>,
}
impl SitesRewritePlan {
    pub fn original_bytes(&self) -> &[u8] {
        &self.original_bytes
    }
}

pub enum VerifiedExecPlan {
    Identity(Arc<VerifiedSource>),
    Translated(Arc<VerifiedArtifact>),
    Rewrite(Arc<SitesRewritePlan>),
}

struct Witness {
    output: Option<Arc<[u8]>>,
    sites: Arc<super::Sites>,
    expected_sha: String,
    identity: bool,
}
// A digest is calculated only after VerifiedSource supplies the complete range.
// Weak entries do not retain every large DSO after its mapping owner retires.
type MemoKey = (String, u32, u32);
static MEMO: LazyLock<Mutex<HashMap<MemoKey, Weak<Witness>>>> = LazyLock::new(Default::default);

pub fn prepare_verified_exec(
    source: Arc<VerifiedSource>,
    host: &Path,
    offset: u64,
    length: u64,
) -> Result<VerifiedExecPlan, Errno> {
    prepare(
        source,
        host,
        offset,
        length,
        &super::rt().caches,
        super::rt().ctr_el0,
    )
}

/// Public mmap retains the original ELF header/segment layout even when a
/// whole-file translated artifact could have been generated.
pub fn prepare_verified_rewrite(
    source: Arc<VerifiedSource>,
    host: &Path,
    range: &Range<u64>,
) -> Result<Arc<SitesRewritePlan>, Errno> {
    let length = range.end.checked_sub(range.start).ok_or(errno::EIO)?;
    match prepare_with_rewrite(
        source,
        host,
        range.start,
        length,
        &super::rt().caches,
        super::rt().ctr_el0,
        true,
    )? {
        VerifiedExecPlan::Rewrite(plan) => Ok(plan),
        _ => unreachable!("forced original-layout rewrite"),
    }
}

pub struct VerifiedMemberRewrite {
    /// Exact stored member range in the authenticated archive.
    pub range: Range<u64>,
    pub rewrite: Arc<SitesRewritePlan>,
}

/// A public executable archive mapping must identify a valid stored member.
/// Malformed archive structure and protected read failures both remain EIO.
pub fn prepare_verified_member_rewrite(
    source: Arc<VerifiedSource>,
    host: &Path,
    offset: u64,
) -> Result<VerifiedMemberRewrite, Errno> {
    let failure = std::cell::Cell::new(None);
    let read = |at: u64, len: usize| {
        let mut bytes = vec![0; len];
        match source.read_exact_at(&mut bytes, at) {
            Ok(()) => Some(bytes),
            Err(error) => {
                failure.set(Some(error));
                None
            }
        }
    };
    let members = crate::zip::members(source.len()?, read);
    if let Some(error) = failure.get() {
        return Err(error);
    }
    let members = members.ok_or(errno::EIO)?;
    let member = crate::zip::stored_at(&members, offset, read);
    if let Some(error) = failure.get() {
        return Err(error);
    }
    let (start, length) = member.ok_or(errno::EIO)?;
    let range = start..start.checked_add(length).ok_or(errno::EIO)?;
    let rewrite = prepare_verified_rewrite(source, host, &range)?;
    Ok(VerifiedMemberRewrite { range, rewrite })
}

fn prepare(
    source: Arc<VerifiedSource>,
    host: &Path,
    offset: u64,
    length: u64,
    caches: &[Cache],
    ctr: u32,
) -> Result<VerifiedExecPlan, Errno> {
    prepare_with_rewrite(source, host, offset, length, caches, ctr, false)
}

fn prepare_with_rewrite(
    source: Arc<VerifiedSource>,
    _host: &Path,
    offset: u64,
    length: u64,
    caches: &[Cache],
    ctr: u32,
    force_rewrite: bool,
) -> Result<VerifiedExecPlan, Errno> {
    if offset.checked_add(length).ok_or(errno::EIO)? > source.len()? {
        return Err(errno::EIO);
    }
    let mut bytes = vec![0; usize::try_from(length).map_err(|_| errno::EIO)?];
    // No Option conversion: corrupt blocks and short reads remain EIO.
    source.read_exact_at(&mut bytes, offset)?;
    let digest = xlate::sha256_hex(&bytes);
    let key = (digest.clone(), xlate::VERSION, ctr);
    let witness = MEMO.lock().unwrap().get(&key).and_then(Weak::upgrade);
    let witness = match witness {
        Some(witness) => witness,
        None => {
            let elf = xlate::elf::parse(&bytes).map_err(|_| errno::ENOEXEC)?;
            let analysis = xlate::analyze(&elf);
            let sites = Arc::new(super::Sites {
                sites: analysis
                    .sites
                    .iter()
                    .map(|site| (site.offset, site.kind, site.rt))
                    .collect(),
                fips: analysis.fips,
            });
            let translated = xlate::translate(&bytes, &xlate::Options { ctr_el0: ctr })
                .map_err(|_| errno::ENOEXEC)?;
            let (output, identity) = match translated.outcome {
                xlate::Outcome::Identity => (None, true),
                xlate::Outcome::Translated(output) => {
                    (Some(Arc::<[u8]>::from(output.to_bytes())), false)
                }
                xlate::Outcome::Unsupported(_) => (None, false),
            };
            let expected_sha = output.as_ref().map_or_else(
                || xlate::sha256_hex(&bytes),
                |output| xlate::sha256_hex(output),
            );
            let witness = Arc::new(Witness {
                output,
                sites,
                expected_sha,
                identity,
            });
            let mut memo = MEMO.lock().unwrap();
            memo.retain(|_, value| value.strong_count() != 0);
            memo.insert(key, Arc::downgrade(&witness));
            witness
        }
    };
    // Every current cache candidate is checked against PURE computation. An
    // attacker editing both the cache file and its claimed SHA cannot pass.
    for cache in caches {
        cache.verify_derivative_candidate(
            &digest,
            ctr,
            witness.output.as_deref(),
            witness.identity,
        )?;
        cache.verify_sites_candidate(&digest, &witness.sites.sites)?;
    }
    let binding = DerivationBinding {
        original_identity: source.identity(),
        source_offset: offset,
        source_length: length,
        original_sha256: digest,
        translator_version: xlate::VERSION,
        ctr_el0: ctr,
        expected_output_sha256: witness.expected_sha.clone(),
        expected_sites_sha256: xlate::sha256_hex(&cache::encode_sites(&witness.sites.sites)),
    };
    if !force_rewrite && witness.identity {
        return Ok(VerifiedExecPlan::Identity(source));
    }
    if let Some(output) = &witness.output
        && !force_rewrite
    {
        return Ok(VerifiedExecPlan::Translated(Arc::new(VerifiedArtifact {
            original: source,
            binding,
            bytes: output.clone(),
            _witness: witness.clone(),
            fips: witness.sites.fips.clone(),
        })));
    }
    Ok(VerifiedExecPlan::Rewrite(Arc::new(SitesRewritePlan {
        original: source,
        binding,
        sites: witness.sites.clone(),
        load_segments: xlate::elf::parse(&bytes)
            .map_err(|_| errno::ENOEXEC)?
            .phdrs
            .into_iter()
            .filter(|header| header.p_type == xlate::elf::PT_LOAD)
            .collect(),
        original_bytes: Arc::from(bytes),
        _witness: witness,
    })))
}

/// Archive header reads use the same authenticated original owner. Invalid ZIP
/// structure is absence; an actual protected read failure is never absence.
pub fn prepare_verified_member(
    source: Arc<VerifiedSource>,
    host: &Path,
    offset: u64,
) -> Result<Option<VerifiedExecPlan>, Errno> {
    let failure = std::cell::Cell::new(None);
    let read = |at: u64, len: usize| {
        let mut bytes = vec![0; len];
        match source.read_exact_at(&mut bytes, at) {
            Ok(()) => Some(bytes),
            Err(error) => {
                failure.set(Some(error));
                None
            }
        }
    };
    let members = crate::zip::members(source.len()?, read);
    if let Some(error) = failure.get() {
        return Err(error);
    }
    let Some(members) = members else {
        return Ok(None);
    };
    let range = crate::zip::stored_at(&members, offset, read);
    if let Some(error) = failure.get() {
        return Err(error);
    }
    let Some((start, length)) = range else {
        return Ok(None);
    };
    prepare_verified_exec(source, host, start, length).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, File},
        os::{
            fd::AsFd,
            unix::fs::{FileExt, PermissionsExt},
        },
    };
    fn elf(align: u64) -> Vec<u8> {
        let mut bytes = vec![0; 0x4008];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&183u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        bytes[24..32].copy_from_slice(&0x4000u64.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&2u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
        for (index, flags, offset, length) in [(0, 4, 0, 176), (1, 5, 0x4000, 8)] {
            let header = xlate::elf::Phdr {
                p_type: 1,
                p_flags: flags,
                p_offset: offset,
                p_vaddr: offset,
                p_paddr: offset,
                p_filesz: length,
                p_memsz: length,
                p_align: align,
            };
            bytes[64 + index * 56..120 + index * 56].copy_from_slice(&header.to_bytes());
        }
        bytes[0x4000..0x4004].copy_from_slice(&0xd4000001u32.to_le_bytes());
        bytes[0x4004..0x4008].copy_from_slice(&0xd65f03c0u32.to_le_bytes());
        bytes
    }
    fn protected(root: &Path, bytes: &[u8]) -> Arc<VerifiedSource> {
        use aim_storage::{
            fsverity::{BuildOptions, Identity, Store},
            private_fd::PrivateFd,
        };
        fs::write(root.join("original"), bytes).unwrap();
        let file = File::open(root.join("original")).unwrap();
        let store = Arc::new(Store::new(&root.join("proof"), &root.join("runtime")).unwrap());
        let admission = store.lock_inode(&file).unwrap();
        let guard = admission.begin_enable().unwrap();
        drop(admission);
        let prepared = guard
            .build(
                BuildOptions::new(1, 4096, vec![], 16384, 4096).unwrap(),
                &[],
                || false,
            )
            .unwrap();
        drop(guard.commit(prepared).unwrap());
        let identity = Identity::from_fd(file.as_fd()).unwrap();
        let proof = Arc::new(store.lookup(identity).unwrap().unwrap());
        Arc::new(VerifiedSource::from_proof(PrivateFd::adopt(file.into()).unwrap(), proof).unwrap())
    }
    fn directory(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aim-verified-derivative-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn protected_derivative_rejects_output_and_forged_meta_and_original_corruption() {
        let root = directory("cache");
        let bytes = elf(16384);
        let source = protected(&root, &bytes);
        let cache = Cache::new(root.join("cache"));
        let options = xlate::Options::default();
        cache
            .translate_file(&root.join("original"), &options)
            .unwrap()
            .unwrap();
        let plan = prepare(
            source.clone(),
            &root.join("original"),
            0,
            bytes.len() as u64,
            &[cache.clone()],
            options.ctr_el0,
        )
        .unwrap();
        let VerifiedExecPlan::Translated(artifact) = plan else {
            panic!("synthetic ELF must produce actual derivative")
        };
        assert_ne!(artifact.bytes(), bytes);
        assert_eq!(
            artifact.binding.expected_output_sha256,
            xlate::sha256_hex(artifact.bytes())
        );
        let dir = cache.entry_dir(&xlate::key_for_digest(&xlate::sha256_hex(&bytes)));
        let output = dir.join("elf");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o644)).unwrap();
        let mut forged = fs::read(&output).unwrap();
        forged[0x4000] ^= 1;
        fs::write(&output, &forged).unwrap();
        let meta = dir.join("meta");
        fs::set_permissions(&meta, fs::Permissions::from_mode(0o644)).unwrap();
        let text = fs::read_to_string(&meta)
            .unwrap()
            .lines()
            .map(|line| {
                if line.starts_with("output_sha256=") {
                    format!("output_sha256={}", xlate::sha256_hex(&forged))
                } else {
                    line.into()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&meta, text).unwrap();
        assert!(matches!(
            prepare(
                source.clone(),
                &root.join("original"),
                0,
                bytes.len() as u64,
                &[cache.clone()],
                options.ctr_el0
            ),
            Err(errno::EIO)
        ));
        File::options()
            .write(true)
            .open(root.join("original"))
            .unwrap()
            .write_all_at(&[0], 0x4000)
            .unwrap();
        assert!(matches!(
            prepare(
                source.clone(),
                &root.join("original"),
                0,
                bytes.len() as u64,
                &[],
                options.ctr_el0
            ),
            Err(errno::EIO)
        ));
        drop(artifact);
        drop(source);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unsupported_alignment_retains_authenticated_sites_and_hash_dependencies() {
        let root = directory("sites");
        let bytes = elf(4096);
        let source = protected(&root, &bytes);
        let plan = prepare(
            source.clone(),
            &root.join("original"),
            0,
            bytes.len() as u64,
            &[],
            xlate::Options::default().ctr_el0,
        )
        .unwrap();
        let VerifiedExecPlan::Rewrite(plan) = plan else {
            panic!("4KiB ELF requires authenticated runtime rewrite")
        };
        assert!(!plan.sites.sites.is_empty());
        assert_eq!(plan.original_bytes(), bytes);
        assert_eq!(
            plan.binding.expected_sites_sha256,
            xlate::sha256_hex(&cache::encode_sites(&plan.sites.sites))
        );
        drop(plan);
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pure_translation_still_supplies_original_layout_authenticated_rewrite() {
        let root = directory("mmap");
        let mut bytes = elf(16384);
        bytes[0x4000..0x4004].copy_from_slice(&(crate::a64::MRS_TPIDR_EL0 | 3).to_le_bytes());
        let source = protected(&root, &bytes);
        assert!(matches!(
            prepare(
                source.clone(),
                &root.join("original"),
                0,
                bytes.len() as u64,
                &[],
                xlate::Options::default().ctr_el0
            )
            .unwrap(),
            VerifiedExecPlan::Translated(_)
        ));
        let plan = prepare_verified_rewrite(
            source.clone(),
            &root.join("original"),
            &(0..bytes.len() as u64),
        )
        .unwrap();
        assert_eq!(plan.original_bytes(), bytes);
        assert!(!plan.sites.sites.is_empty());
        assert_eq!(plan.sites.sites[0].0, 0x4000);
        assert_eq!(plan.load_segments.len(), 2);
        assert_eq!(plan.load_segments[1].p_align, 16384);
        assert_eq!(plan.binding.original_sha256, xlate::sha256_hex(&bytes));
        File::options()
            .write(true)
            .open(root.join("original"))
            .unwrap()
            .write_all_at(&[0], 0x4000)
            .unwrap();
        assert!(matches!(
            prepare_verified_rewrite(
                source.clone(),
                &root.join("original"),
                &(0..bytes.len() as u64)
            ),
            Err(errno::EIO)
        ));
        drop(plan);
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }
    /// A real stored ZIP member, with CRC32 and a valid page-alignment extra
    /// field. The central directory describes the same immutable ELF bytes.
    fn stored_zip(bytes: &[u8], start: usize) -> Vec<u8> {
        let name = b"lib/arm64-v8a/libverified.so";
        let extra_len = start - 30 - name.len();
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
            }
        }
        let crc = !crc;
        let mut local = [0u8; 30];
        local[..4].copy_from_slice(b"PK\x03\x04");
        local[4..6].copy_from_slice(&20u16.to_le_bytes());
        local[14..18].copy_from_slice(&crc.to_le_bytes());
        local[18..22].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        local[22..26].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        local[28..30].copy_from_slice(&(extra_len as u16).to_le_bytes());
        let mut archive = local.to_vec();
        archive.extend_from_slice(name);
        archive.extend_from_slice(&0xffffu16.to_le_bytes());
        archive.extend_from_slice(&((extra_len - 4) as u16).to_le_bytes());
        archive.resize(start, 0);
        archive.extend_from_slice(bytes);
        let directory_at = archive.len() as u32;
        let mut central = [0u8; 46];
        central[..4].copy_from_slice(b"PK\x01\x02");
        central[4..6].copy_from_slice(&20u16.to_le_bytes());
        central[6..8].copy_from_slice(&20u16.to_le_bytes());
        central[16..20].copy_from_slice(&crc.to_le_bytes());
        central[20..24].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        central[24..28].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        archive.extend_from_slice(&central);
        archive.extend_from_slice(name);
        let mut end = [0u8; 22];
        end[..4].copy_from_slice(b"PK\x05\x06");
        end[8..10].copy_from_slice(&1u16.to_le_bytes());
        end[10..12].copy_from_slice(&1u16.to_le_bytes());
        end[12..16].copy_from_slice(&((46 + name.len()) as u32).to_le_bytes());
        end[16..20].copy_from_slice(&directory_at.to_le_bytes());
        archive.extend_from_slice(&end);
        archive
    }

    #[test]
    fn stored_zip_member_retains_exact_original_layout_and_rejects_tampering() {
        let root = directory("zip");
        let mut original = elf(16384);
        original[0x4000..0x4004].copy_from_slice(&(crate::a64::MRS_TPIDR_EL0 | 7).to_le_bytes());
        let start = 0x4000;
        let archive = stored_zip(&original, start);
        let source = protected(&root, &archive);
        // Request lies inside the member's executable segment, not its header.
        let member = prepare_verified_member_rewrite(
            source.clone(),
            &root.join("original"),
            (start + 0x4000) as u64,
        )
        .unwrap();
        assert_eq!(member.range, start as u64..(start + original.len()) as u64);
        assert_eq!(member.rewrite.original_bytes(), original);
        assert_eq!(member.rewrite.binding.source_offset, start as u64);
        assert_eq!(member.rewrite.binding.source_length, original.len() as u64);
        assert_eq!(
            member.rewrite.binding.original_sha256,
            xlate::sha256_hex(&original)
        );
        assert_eq!(member.rewrite.load_segments.len(), 2);
        assert_eq!(member.rewrite.load_segments[1].p_offset, 0x4000);
        assert_eq!(member.rewrite.load_segments[1].p_align, 16384);
        assert_eq!(member.rewrite.sites.sites[0].0, 0x4000);
        assert_eq!(member.rewrite.sites.sites[0].2, 7);
        // Change the protected archive's member instruction without changing
        // the proof. Header/member discovery must not turn the error into None.
        File::options()
            .write(true)
            .open(root.join("original"))
            .unwrap()
            .write_all_at(&[0], (start + 0x4000) as u64)
            .unwrap();
        assert!(matches!(
            prepare_verified_member_rewrite(
                source.clone(),
                &root.join("original"),
                (start + 0x4000) as u64
            ),
            Err(errno::EIO)
        ));
        drop(member);
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }
}

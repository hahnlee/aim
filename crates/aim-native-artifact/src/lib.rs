//! Native artifact admission and graph-selection policy.
//!
//! Android keeps the guest-visible `lib*.so` identity. A Darwin accelerator is
//! a separate, install-time product and is admitted only when the complete
//! native graph is an arm64 macOS Mach-O graph with an exact conversion
//! contract. The resolver never creates a mixed ELF/Mach-O dependency graph.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

mod conversion;

pub use conversion::{ConversionOutcome, ConversionRequest, prepare_complete_darwin_graph};

const ELF_MAGIC: &[u8; 4] = b"\x7fELF";
const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MH_DYLIB: u32 = 6;
const LC_ID_DYLIB: u32 = 0x0d;
const LC_LOAD_DYLIB: u32 = 0x0c;
const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
const LC_REEXPORT_DYLIB: u32 = 0x8000_001f;
const LC_LAZY_LOAD_DYLIB: u32 = 0x20;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const LC_BUILD_VERSION: u32 = 0x32;
const PLATFORM_MACOS: u32 = 1;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
pub const CONTRACT_VERSION: &str = "aim-complete-darwin-v1";

pub type Result<T> = std::result::Result<T, AdmissionError>;

#[derive(Debug)]
pub enum AdmissionError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Contract(String),
    Format(String),
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Contract(message) => write!(formatter, "Darwin contract error: {message}"),
            Self::Format(message) => write!(formatter, "native artifact format error: {message}"),
        }
    }
}

impl std::error::Error for AdmissionError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompleteDarwinContract {
    pub logical_soname: String,
    pub apk_sha256: String,
    pub source_elf_sha256: String,
    pub dylib_sha256: String,
    pub runtime_abi: String,
}

impl CompleteDarwinContract {
    pub fn parse(input: &str) -> Result<Self> {
        let mut lines = input.lines();
        if lines.next() != Some(CONTRACT_VERSION) {
            return Err(AdmissionError::Contract(
                "missing or unsupported contract version".to_owned(),
            ));
        }
        let mut fields = BTreeMap::new();
        for line in lines {
            let (key, value) = line.split_once('=').ok_or_else(|| {
                AdmissionError::Contract("contract line has no '=' separator".to_owned())
            })?;
            if key.is_empty() || value.is_empty() || fields.insert(key, value).is_some() {
                return Err(AdmissionError::Contract(
                    "empty or duplicate contract field".to_owned(),
                ));
            }
        }
        let expected = BTreeSet::from([
            "apk-sha256",
            "architecture",
            "darwin-complete",
            "dylib-sha256",
            "logical-soname",
            "platform",
            "runtime-abi",
            "source-elf-sha256",
        ]);
        if fields.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(AdmissionError::Contract(
                "contract field set is not exact".to_owned(),
            ));
        }
        if fields["darwin-complete"] != "true"
            || fields["architecture"] != "arm64"
            || fields["platform"] != "macos"
        {
            return Err(AdmissionError::Contract(
                "artifact is not declared complete arm64 Darwin".to_owned(),
            ));
        }
        let contract = Self {
            logical_soname: fields["logical-soname"].to_owned(),
            apk_sha256: fields["apk-sha256"].to_owned(),
            source_elf_sha256: fields["source-elf-sha256"].to_owned(),
            dylib_sha256: fields["dylib-sha256"].to_owned(),
            runtime_abi: fields["runtime-abi"].to_owned(),
        };
        if !valid_soname(&contract.logical_soname)
            || !valid_hash(&contract.apk_sha256)
            || !valid_hash(&contract.source_elf_sha256)
            || !valid_hash(&contract.dylib_sha256)
            || contract.runtime_abi.is_empty()
        {
            return Err(AdmissionError::Contract(
                "contract identity field is malformed".to_owned(),
            ));
        }
        Ok(contract)
    }

    pub fn encode(&self) -> String {
        format!(
            "{CONTRACT_VERSION}\nlogical-soname={}\napk-sha256={}\nsource-elf-sha256={}\ndylib-sha256={}\nruntime-abi={}\ndarwin-complete=true\narchitecture=arm64\nplatform=macos\n",
            self.logical_soname,
            self.apk_sha256,
            self.source_elf_sha256,
            self.dylib_sha256,
            self.runtime_abi,
        )
    }
}

#[derive(Clone, Debug)]
pub struct LibraryCandidate {
    pub logical_soname: String,
    pub elf_path: PathBuf,
    pub dylib_path: Option<PathBuf>,
    pub contract_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphSelection {
    CompleteDarwin(Vec<PathBuf>),
    AndroidElf(Vec<PathBuf>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Publication {
    Published,
    Existing,
}

/// Validate and atomically publish one complete Darwin graph cache directory.
///
/// The staging directory must be a sibling of `destination`, so the exclusive
/// rename cannot cross filesystems. A racing winner is accepted only after its
/// complete immutable graph is independently revalidated.
pub fn publish_complete_darwin_graph(
    apk_sha256: &str,
    runtime_abi: &str,
    elf_directory: &Path,
    staging_directory: &Path,
    destination: &Path,
) -> Result<Publication> {
    let parent = destination.parent().ok_or_else(|| {
        AdmissionError::Contract("Darwin cache destination has no parent".to_owned())
    })?;
    if staging_directory.parent() != Some(parent) || staging_directory == destination {
        return Err(AdmissionError::Contract(
            "Darwin cache staging directory must be a destination sibling".to_owned(),
        ));
    }
    require_plain_directory(staging_directory)?;
    require_exact_graph_files(elf_directory, staging_directory)?;
    match resolve_directory_graph(apk_sha256, runtime_abi, elf_directory, staging_directory)? {
        GraphSelection::CompleteDarwin(_) => {}
        GraphSelection::AndroidElf(_) => {
            return Err(AdmissionError::Contract(
                "staging directory does not contain a complete Darwin graph".to_owned(),
            ));
        }
    }

    seal_and_sync_directory(staging_directory)?;
    match rename_exclusive(staging_directory, destination) {
        Ok(()) => {
            sync_directory(parent)?;
            Ok(Publication::Published)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            require_plain_directory(destination)?;
            require_exact_graph_files(elf_directory, destination)?;
            match resolve_directory_graph(apk_sha256, runtime_abi, elf_directory, destination)? {
                GraphSelection::CompleteDarwin(_) => Ok(Publication::Existing),
                GraphSelection::AndroidElf(_) => Err(AdmissionError::Contract(
                    "existing Darwin cache is not a complete graph".to_owned(),
                )),
            }
        }
        Err(source) => Err(AdmissionError::Io {
            path: destination.to_path_buf(),
            source,
        }),
    }
}

pub fn resolve_directory_graph(
    apk_sha256: &str,
    runtime_abi: &str,
    elf_directory: &Path,
    darwin_directory: &Path,
) -> Result<GraphSelection> {
    let mut elf_paths = direct_regular_files(elf_directory, ".so")?;
    if elf_paths.is_empty() {
        return Err(AdmissionError::Contract(
            "installed ELF directory contains no libraries".to_owned(),
        ));
    }
    elf_paths.sort();
    let darwin_exists = darwin_directory.is_dir();
    let candidates = elf_paths
        .into_iter()
        .map(|elf_path| {
            let logical_soname = elf_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    AdmissionError::Contract("installed ELF SONAME is not UTF-8".to_owned())
                })?
                .to_owned();
            let stem = logical_soname.strip_suffix(".so").ok_or_else(|| {
                AdmissionError::Contract("installed ELF has an invalid suffix".to_owned())
            })?;
            let dylib_path = darwin_directory.join(format!("{stem}.dylib"));
            let contract_path = darwin_directory.join(format!("{stem}.dylib.contract"));
            let has_dylib = dylib_path
                .try_exists()
                .map_err(|source| AdmissionError::Io {
                    path: dylib_path.clone(),
                    source,
                })?;
            let has_contract = contract_path
                .try_exists()
                .map_err(|source| AdmissionError::Io {
                    path: contract_path.clone(),
                    source,
                })?;
            if has_dylib != has_contract {
                return Err(AdmissionError::Contract(format!(
                    "partial Darwin artifact installation for {logical_soname}"
                )));
            }
            Ok(LibraryCandidate {
                logical_soname,
                elf_path,
                dylib_path: (darwin_exists && has_dylib).then_some(dylib_path),
                contract_path: (darwin_exists && has_contract).then_some(contract_path),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    resolve_graph(apk_sha256, runtime_abi, &candidates)
}

/// Resolve one closed native graph. Darwin is all-or-nothing: an absent
/// accelerator falls back to the original ELF graph, while a present but
/// invalid accelerator is an installation-integrity error.
pub fn resolve_graph(
    apk_sha256: &str,
    runtime_abi: &str,
    candidates: &[LibraryCandidate],
) -> Result<GraphSelection> {
    if !valid_hash(apk_sha256) || runtime_abi.is_empty() || candidates.is_empty() {
        return Err(AdmissionError::Contract(
            "graph request identity is malformed".to_owned(),
        ));
    }
    let mut names = BTreeSet::new();
    let mut elf_paths = Vec::with_capacity(candidates.len());
    let mut dylib_paths = Vec::with_capacity(candidates.len());
    let mut all_darwin = true;
    let expected_dylibs = candidates
        .iter()
        .map(|candidate| {
            candidate
                .logical_soname
                .strip_suffix(".so")
                .map(|stem| format!("{stem}.dylib"))
                .ok_or_else(|| {
                    AdmissionError::Contract("graph has an invalid logical SONAME".to_owned())
                })
        })
        .collect::<Result<BTreeSet<_>>>()?;

    for candidate in candidates {
        if !valid_soname(&candidate.logical_soname)
            || !names.insert(candidate.logical_soname.as_str())
        {
            return Err(AdmissionError::Contract(
                "graph has a malformed or duplicate logical SONAME".to_owned(),
            ));
        }
        let elf = read(&candidate.elf_path)?;
        validate_android_elf(&elf)?;
        elf_paths.push(candidate.elf_path.clone());

        match (&candidate.dylib_path, &candidate.contract_path) {
            (None, None) => all_darwin = false,
            (Some(dylib_path), Some(contract_path)) => {
                let dylib = read(dylib_path)?;
                validate_complete_darwin_macho(&dylib)?;
                let expected_id = format!(
                    "@loader_path/{}.dylib",
                    candidate.logical_soname.trim_end_matches(".so")
                );
                validate_darwin_graph_edges(&dylib, &expected_id, &expected_dylibs)?;
                let contract_text =
                    fs::read_to_string(contract_path).map_err(|source| AdmissionError::Io {
                        path: contract_path.clone(),
                        source,
                    })?;
                let contract = CompleteDarwinContract::parse(&contract_text)?;
                if contract.logical_soname != candidate.logical_soname
                    || contract.apk_sha256 != apk_sha256
                    || contract.runtime_abi != runtime_abi
                    || contract.source_elf_sha256 != sha256(&elf)
                    || contract.dylib_sha256 != sha256(&dylib)
                {
                    return Err(AdmissionError::Contract(
                        "Darwin artifact does not match its APK/ELF/runtime identity".to_owned(),
                    ));
                }
                dylib_paths.push(dylib_path.clone());
            }
            _ => {
                return Err(AdmissionError::Contract(
                    "Darwin artifact and contract must be installed together".to_owned(),
                ));
            }
        }
    }

    if all_darwin {
        Ok(GraphSelection::CompleteDarwin(dylib_paths))
    } else {
        Ok(GraphSelection::AndroidElf(elf_paths))
    }
}

pub fn validate_android_elf(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 64
        || &bytes[..4] != ELF_MAGIC
        || bytes[4] != 2
        || bytes[5] != 1
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 183
    {
        return Err(AdmissionError::Format(
            "source is not ELF64 little-endian AArch64".to_owned(),
        ));
    }
    Ok(())
}

pub fn validate_complete_darwin_macho(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 32 || le32(bytes, 0)? != MH_MAGIC_64 {
        return Err(AdmissionError::Format(
            "accelerator is not a thin 64-bit Mach-O".to_owned(),
        ));
    }
    if le32(bytes, 4)? != CPU_TYPE_ARM64 || le32(bytes, 8)? & 0x00ff_ffff != 0 {
        return Err(AdmissionError::Format(
            "accelerator is not baseline Darwin arm64".to_owned(),
        ));
    }
    if le32(bytes, 12)? != MH_DYLIB {
        return Err(AdmissionError::Format(
            "accelerator Mach-O is not MH_DYLIB".to_owned(),
        ));
    }
    let commands = le32(bytes, 16)? as usize;
    let command_bytes = le32(bytes, 20)? as usize;
    let commands_end = 32_usize
        .checked_add(command_bytes)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| AdmissionError::Format("Mach-O load commands exceed file".to_owned()))?;
    let mut offset = 32_usize;
    let mut id = None;
    let mut has_macos = false;
    let mut has_code_signature = false;
    for _ in 0..commands {
        let cmd = le32(bytes, offset)?;
        let size = le32(bytes, offset + 4)? as usize;
        let end = offset
            .checked_add(size)
            .filter(|end| size >= 8 && *end <= commands_end)
            .ok_or_else(|| AdmissionError::Format("invalid Mach-O load command".to_owned()))?;
        if cmd == LC_ID_DYLIB {
            if id.is_some() {
                return Err(AdmissionError::Format(
                    "Mach-O has duplicate LC_ID_DYLIB commands".to_owned(),
                ));
            }
            id = Some(dylib_command_name(bytes, offset, end)?);
        } else if cmd == LC_BUILD_VERSION {
            if size < 24 || le32(bytes, offset + 8)? != PLATFORM_MACOS {
                return Err(AdmissionError::Format(
                    "accelerator targets a non-macOS platform".to_owned(),
                ));
            }
            has_macos = true;
        } else if matches!(
            cmd,
            LC_LOAD_DYLIB
                | LC_LOAD_WEAK_DYLIB
                | LC_REEXPORT_DYLIB
                | LC_LAZY_LOAD_DYLIB
                | LC_LOAD_UPWARD_DYLIB
        ) {
            if size < 24 {
                return Err(AdmissionError::Format(
                    "truncated dylib load command".to_owned(),
                ));
            }
            let name = dylib_command_name(bytes, offset, end)?;
            if name.ends_with(".so") || name.contains("/lib/arm64-v8a/") {
                return Err(AdmissionError::Format(
                    "Mach-O still depends on an Android native artifact".to_owned(),
                ));
            }
            if !valid_darwin_dependency(&name) {
                return Err(AdmissionError::Format(format!(
                    "Mach-O has an untrusted Darwin dependency: {name}"
                )));
            }
        } else if cmd == LC_CODE_SIGNATURE {
            if has_code_signature || size < 16 {
                return Err(AdmissionError::Format(
                    "Mach-O has an invalid code-signature command".to_owned(),
                ));
            }
            let data_offset = le32(bytes, offset + 8)? as usize;
            let data_size = le32(bytes, offset + 12)? as usize;
            let signature_end = data_offset
                .checked_add(data_size)
                .filter(|signature_end| data_size >= 12 && *signature_end <= bytes.len())
                .ok_or_else(|| {
                    AdmissionError::Format("Mach-O code signature exceeds file".to_owned())
                })?;
            let declared_size = be32(bytes, data_offset + 4)? as usize;
            if be32(bytes, data_offset)? != CSMAGIC_EMBEDDED_SIGNATURE
                || declared_size < 12
                || declared_size > data_size
                || bytes[data_offset + declared_size..signature_end]
                    .iter()
                    .any(|byte| *byte != 0)
                || signature_end != bytes.len()
            {
                return Err(AdmissionError::Format(
                    "Mach-O embedded code signature is malformed or not terminal".to_owned(),
                ));
            }
            has_code_signature = true;
        }
        offset = end;
    }
    let id = id.ok_or_else(|| AdmissionError::Format("Mach-O lacks LC_ID_DYLIB".to_owned()))?;
    if offset != commands_end || !has_macos || !has_code_signature {
        return Err(AdmissionError::Format(
            "Mach-O lacks an exact command table, macOS build version, or code signature"
                .to_owned(),
        ));
    }
    if !id.starts_with("@loader_path/lib") || !id.ends_with(".dylib") {
        return Err(AdmissionError::Format(
            "Mach-O install name is not a direct @loader_path dylib".to_owned(),
        ));
    }
    Ok(())
}

fn validate_darwin_graph_edges(
    bytes: &[u8],
    expected_id: &str,
    expected_dylibs: &BTreeSet<String>,
) -> Result<()> {
    let commands = le32(bytes, 16)? as usize;
    let command_bytes = le32(bytes, 20)? as usize;
    let commands_end = 32_usize
        .checked_add(command_bytes)
        .ok_or_else(|| AdmissionError::Format("Mach-O command table overflows".to_owned()))?;
    let mut offset = 32_usize;
    let mut actual_id = None;
    for _ in 0..commands {
        let cmd = le32(bytes, offset)?;
        let size = le32(bytes, offset + 4)? as usize;
        let end = offset
            .checked_add(size)
            .filter(|end| *end <= commands_end)
            .ok_or_else(|| {
                AdmissionError::Format("invalid Mach-O graph load command".to_owned())
            })?;
        if cmd == LC_ID_DYLIB {
            actual_id = Some(dylib_command_name(bytes, offset, end)?);
        } else if matches!(
            cmd,
            LC_LOAD_DYLIB
                | LC_LOAD_WEAK_DYLIB
                | LC_REEXPORT_DYLIB
                | LC_LAZY_LOAD_DYLIB
                | LC_LOAD_UPWARD_DYLIB
        ) {
            let dependency = dylib_command_name(bytes, offset, end)?;
            if let Some(leaf) = dependency.strip_prefix("@loader_path/")
                && !expected_dylibs.contains(leaf)
            {
                return Err(AdmissionError::Contract(format!(
                    "Darwin dylib dependency is outside the converted graph: {dependency}"
                )));
            }
        }
        offset = end;
    }
    if actual_id.as_deref() != Some(expected_id) {
        return Err(AdmissionError::Contract(format!(
            "Darwin dylib install name does not match logical SONAME: expected {expected_id}"
        )));
    }
    Ok(())
}

fn dylib_command_name(bytes: &[u8], offset: usize, end: usize) -> Result<String> {
    if end.saturating_sub(offset) < 24 {
        return Err(AdmissionError::Format("truncated dylib command".to_owned()));
    }
    let name_offset = le32(bytes, offset + 8)? as usize;
    let name_start = offset
        .checked_add(name_offset)
        .filter(|start| *start < end)
        .ok_or_else(|| AdmissionError::Format("invalid dylib name offset".to_owned()))?;
    let name_end = bytes[name_start..end]
        .iter()
        .position(|byte| *byte == 0)
        .map(|length| name_start + length)
        .ok_or_else(|| AdmissionError::Format("unterminated dylib name".to_owned()))?;
    std::str::from_utf8(&bytes[name_start..name_end])
        .map(str::to_owned)
        .map_err(|_| AdmissionError::Format("non-UTF-8 dylib name".to_owned()))
}

fn valid_darwin_dependency(name: &str) -> bool {
    (name.starts_with("@loader_path/lib") && name.ends_with(".dylib"))
        || name.starts_with("/usr/lib/")
        || name.starts_with("/System/Library/Frameworks/")
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|source| AdmissionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| AdmissionError::Format("truncated native artifact".to_owned()))?;
    Ok(u32::from_le_bytes(raw.try_into().unwrap()))
}

fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| AdmissionError::Format("truncated native artifact".to_owned()))?;
    Ok(u32::from_be_bytes(raw.try_into().unwrap()))
}

fn valid_soname(value: &str) -> bool {
    value.starts_with("lib")
        && value.ends_with(".so")
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn direct_regular_files(directory: &Path, suffix: &str) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory).map_err(|source| AdmissionError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| AdmissionError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| AdmissionError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(AdmissionError::Contract(format!(
                "native artifact directory contains symlink {}",
                path.display()
            )));
        }
        if metadata.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(suffix))
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn require_exact_graph_files(elf_directory: &Path, darwin_directory: &Path) -> Result<()> {
    let elf_paths = direct_regular_files(elf_directory, ".so")?;
    let mut expected = BTreeSet::new();
    for elf in elf_paths {
        let logical = elf
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AdmissionError::Contract("installed ELF SONAME is not UTF-8".to_owned())
            })?;
        let stem = logical.strip_suffix(".so").ok_or_else(|| {
            AdmissionError::Contract("installed ELF has an invalid suffix".to_owned())
        })?;
        expected.insert(format!("{stem}.dylib"));
        expected.insert(format!("{stem}.dylib.contract"));
    }
    let mut actual = BTreeSet::new();
    let entries = fs::read_dir(darwin_directory).map_err(|source| AdmissionError::Io {
        path: darwin_directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| AdmissionError::Io {
            path: darwin_directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| AdmissionError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AdmissionError::Contract(format!(
                "Darwin cache contains a non-regular entry {}",
                path.display()
            )));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            AdmissionError::Contract("Darwin cache filename is not UTF-8".to_owned())
        })?;
        actual.insert(name);
    }
    if actual != expected {
        return Err(AdmissionError::Contract(
            "Darwin cache file set does not exactly match the ELF graph".to_owned(),
        ));
    }
    Ok(())
}

fn require_plain_directory(directory: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(directory).map_err(|source| AdmissionError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AdmissionError::Contract(format!(
            "{} is not a plain directory",
            directory.display()
        )));
    }
    Ok(())
}

fn seal_and_sync_directory(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory).map_err(|source| AdmissionError::Io {
        path: directory.to_path_buf(),
        source,
    })? {
        let path = entry
            .map_err(|source| AdmissionError::Io {
                path: directory.to_path_buf(),
                source,
            })?
            .path();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).map_err(|source| {
            AdmissionError::Io {
                path: path.clone(),
                source,
            }
        })?;
        fs::File::open(&path)
            .and_then(|file| file.sync_all())
            .map_err(|source| AdmissionError::Io {
                path: path.clone(),
                source,
            })?;
    }
    sync_directory(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o500)).map_err(|source| {
        AdmissionError::Io {
            path: directory.to_path_buf(),
            source,
        }
    })
}

fn sync_directory(directory: &Path) -> Result<()> {
    fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|source| AdmissionError::Io {
            path: directory.to_path_buf(),
            source,
        })
}

#[cfg(target_os = "macos")]
fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};

    unsafe extern "C" {
        fn renamex_np(old: *const c_char, new: *const c_char, flags: u32) -> c_int;
    }
    const RENAME_EXCL: u32 = 0x0000_0004;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in source path"))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in destination path"))?;
    // SAFETY: both C strings remain live for the duration of renamex_np and the
    // Darwin-defined RENAME_EXCL flag does not retain either pointer.
    if unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn rename_exclusive(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "exclusive Darwin cache publication requires macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn contract_requires_explicit_complete_darwin_identity() {
        let input = format!(
            "{CONTRACT_VERSION}\nlogical-soname=libfoo.so\napk-sha256={HASH}\nsource-elf-sha256={HASH}\ndylib-sha256={HASH}\nruntime-abi=v1\ndarwin-complete=true\narchitecture=arm64\nplatform=macos\n"
        );
        assert_eq!(
            CompleteDarwinContract::parse(&input)
                .unwrap()
                .logical_soname,
            "libfoo.so"
        );
        assert!(CompleteDarwinContract::parse(&input.replace("true", "partial")).is_err());
        assert!(CompleteDarwinContract::parse(&(input + "guest-abi=android\n")).is_err());
    }

    #[test]
    fn elf_validator_is_content_based_not_extension_based() {
        let mut elf = vec![0; 64];
        elf[..4].copy_from_slice(ELF_MAGIC);
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
        assert!(validate_android_elf(&elf).is_ok());
        assert!(validate_complete_darwin_macho(&elf).is_err());
    }

    #[test]
    fn macho_rejects_android_dependency() {
        let dylib = synthetic_macho("libchild.so");
        assert!(validate_complete_darwin_macho(&dylib).is_err());
        assert!(
            validate_complete_darwin_macho(&synthetic_macho("/usr/lib/libSystem.B.dylib")).is_ok()
        );
    }

    #[test]
    fn macho_requires_terminal_embedded_code_signature() {
        let mut dylib = synthetic_macho("/usr/lib/libSystem.B.dylib");
        let signature_command = dylib
            .windows(4)
            .position(|bytes| bytes == LC_CODE_SIGNATURE.to_le_bytes())
            .unwrap();
        dylib[signature_command..signature_command + 4].copy_from_slice(&0_u32.to_le_bytes());
        assert!(validate_complete_darwin_macho(&dylib).is_err());
    }

    #[test]
    fn graph_rejects_loader_path_dependency_outside_exact_set() {
        let directory = TestDirectory::new();
        let elf = synthetic_elf();
        let dylib = synthetic_macho("@loader_path/libmissing.dylib");
        let elf_path = directory.path.join("libfirst.so");
        let dylib_path = directory.path.join("libfirst.dylib");
        let contract_path = directory.path.join("libfirst.dylib.contract");
        fs::write(&elf_path, &elf).unwrap();
        fs::write(&dylib_path, &dylib).unwrap();
        fs::write(
            &contract_path,
            CompleteDarwinContract {
                logical_soname: "libfirst.so".to_owned(),
                apk_sha256: HASH.to_owned(),
                source_elf_sha256: sha256(&elf),
                dylib_sha256: sha256(&dylib),
                runtime_abi: "v1".to_owned(),
            }
            .encode(),
        )
        .unwrap();
        assert!(
            resolve_graph(
                HASH,
                "v1",
                &[LibraryCandidate {
                    logical_soname: "libfirst.so".to_owned(),
                    elf_path,
                    dylib_path: Some(dylib_path),
                    contract_path: Some(contract_path),
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn graph_selection_never_mixes_darwin_and_elf() {
        let directory = TestDirectory::new();
        let elf = synthetic_elf();
        let dylib = synthetic_macho("/usr/lib/libSystem.B.dylib");
        let first_elf = directory.path.join("libfirst.so");
        let second_elf = directory.path.join("libsecond.so");
        let first_dylib = directory.path.join("libfirst.dylib");
        let contract = directory.path.join("libfirst.dylib.contract");
        fs::write(&first_elf, &elf).unwrap();
        fs::write(&second_elf, &elf).unwrap();
        fs::write(&first_dylib, &dylib).unwrap();
        fs::write(
            &contract,
            format!(
                "{CONTRACT_VERSION}\nlogical-soname=libfirst.so\napk-sha256={HASH}\nsource-elf-sha256={}\ndylib-sha256={}\nruntime-abi=v1\ndarwin-complete=true\narchitecture=arm64\nplatform=macos\n",
                sha256(&elf),
                sha256(&dylib)
            ),
        )
        .unwrap();
        let selection = resolve_graph(
            HASH,
            "v1",
            &[
                LibraryCandidate {
                    logical_soname: "libfirst.so".to_owned(),
                    elf_path: first_elf.clone(),
                    dylib_path: Some(first_dylib),
                    contract_path: Some(contract),
                },
                LibraryCandidate {
                    logical_soname: "libsecond.so".to_owned(),
                    elf_path: second_elf.clone(),
                    dylib_path: None,
                    contract_path: None,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            selection,
            GraphSelection::AndroidElf(vec![first_elf, second_elf])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn complete_graph_publication_is_exclusive_and_revalidates_winner() {
        let directory = TestDirectory::new();
        let elf_directory = directory.path.join("elf");
        let staging = directory.path.join(".stage-one");
        let destination = directory.path.join("v1");
        fs::create_dir(&elf_directory).unwrap();
        fs::create_dir(&staging).unwrap();
        let elf = synthetic_elf();
        let dylib = synthetic_macho("/usr/lib/libSystem.B.dylib");
        fs::write(elf_directory.join("libfirst.so"), &elf).unwrap();
        write_candidate(&staging, "libfirst", &elf, &dylib);
        assert_eq!(
            publish_complete_darwin_graph(HASH, "v1", &elf_directory, &staging, &destination)
                .unwrap(),
            Publication::Published
        );
        assert!(!staging.exists());

        let second = directory.path.join(".stage-two");
        fs::create_dir(&second).unwrap();
        write_candidate(&second, "libfirst", &elf, &dylib);
        assert_eq!(
            publish_complete_darwin_graph(HASH, "v1", &elf_directory, &second, &destination)
                .unwrap(),
            Publication::Existing
        );
        assert!(second.exists());
    }

    fn write_candidate(directory: &Path, stem: &str, elf: &[u8], dylib: &[u8]) {
        fs::write(directory.join(format!("{stem}.dylib")), dylib).unwrap();
        fs::write(
            directory.join(format!("{stem}.dylib.contract")),
            format!(
                "{CONTRACT_VERSION}\nlogical-soname={stem}.so\napk-sha256={HASH}\nsource-elf-sha256={}\ndylib-sha256={}\nruntime-abi=v1\ndarwin-complete=true\narchitecture=arm64\nplatform=macos\n",
                sha256(elf),
                sha256(dylib)
            ),
        )
        .unwrap();
    }

    fn synthetic_elf() -> Vec<u8> {
        let mut elf = vec![0; 64];
        elf[..4].copy_from_slice(ELF_MAGIC);
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
        elf
    }

    fn synthetic_macho(dependency: &str) -> Vec<u8> {
        let id = "@loader_path/libfirst.dylib";
        let id_size = (24 + id.len() + 1 + 7) & !7;
        let dependency_size = (24 + dependency.len() + 1 + 7) & !7;
        let command_bytes = id_size + 24 + dependency_size + 16;
        let signature_size = 12;
        let mut bytes = vec![0_u8; 32 + command_bytes + signature_size];
        bytes[0..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        bytes[4..8].copy_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
        bytes[12..16].copy_from_slice(&MH_DYLIB.to_le_bytes());
        bytes[16..20].copy_from_slice(&4_u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&(command_bytes as u32).to_le_bytes());
        let mut offset = 32;
        bytes[offset..offset + 4].copy_from_slice(&LC_ID_DYLIB.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&(id_size as u32).to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&24_u32.to_le_bytes());
        bytes[offset + 24..offset + 24 + id.len()].copy_from_slice(id.as_bytes());
        offset += id_size;
        bytes[offset..offset + 4].copy_from_slice(&LC_BUILD_VERSION.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&24_u32.to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&PLATFORM_MACOS.to_le_bytes());
        offset += 24;
        bytes[offset..offset + 4].copy_from_slice(&LC_LOAD_DYLIB.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&(dependency_size as u32).to_le_bytes());
        bytes[offset + 8..offset + 12].copy_from_slice(&24_u32.to_le_bytes());
        bytes[offset + 24..offset + 24 + dependency.len()].copy_from_slice(dependency.as_bytes());
        offset += dependency_size;
        bytes[offset..offset + 4].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        bytes[offset + 4..offset + 8].copy_from_slice(&16_u32.to_le_bytes());
        bytes[offset + 8..offset + 12]
            .copy_from_slice(&((32 + command_bytes) as u32).to_le_bytes());
        bytes[offset + 12..offset + 16].copy_from_slice(&(signature_size as u32).to_le_bytes());
        let signature = 32 + command_bytes;
        bytes[signature..signature + 4].copy_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
        bytes[signature + 4..signature + 8].copy_from_slice(&(signature_size as u32).to_be_bytes());
        bytes
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-native-artifact-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            make_directories_writable(&self.path);
            fs::remove_dir_all(&self.path).unwrap();
        }
    }

    fn make_directories_writable(directory: &Path) {
        let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    make_directories_writable(&entry.path());
                }
            }
        }
    }
}

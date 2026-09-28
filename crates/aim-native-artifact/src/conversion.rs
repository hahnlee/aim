use super::{
    AdmissionError, CompleteDarwinContract, GraphSelection, Publication, Result,
    direct_regular_files, publish_complete_darwin_graph, resolve_directory_graph, sha256,
    validate_android_elf, validate_complete_darwin_macho,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FALLBACK_VERSION: &str = "aim-native-fallback-v1";

pub struct ConversionRequest<'a> {
    pub apk_sha256: &'a str,
    pub runtime_abi: &'a str,
    pub elf_directory: &'a Path,
    pub cache_directory: &'a Path,
    pub converter: Option<&'a Path>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversionOutcome {
    CompleteDarwin {
        publication: Publication,
        libraries: Vec<PathBuf>,
    },
    AndroidElf {
        libraries: Vec<PathBuf>,
        cached_failure: bool,
        reason: &'static str,
    },
}

/// Attempt one whole-graph conversion and publish only a fully admitted result.
///
/// The external converter is deliberately untrusted: it receives a private empty
/// directory and may emit only one `lib*.dylib` for every installed `lib*.so`.
/// Rust creates the identity contracts, validates the complete set, and performs
/// exclusive publication. Any converter failure or incomplete staging tree is
/// deleted before the original Android ELF graph is selected.
pub fn prepare_complete_darwin_graph(request: &ConversionRequest<'_>) -> Result<ConversionOutcome> {
    let elf_paths = source_graph(request.elf_directory)?;
    if request
        .cache_directory
        .try_exists()
        .map_err(|source| AdmissionError::Io {
            path: request.cache_directory.to_path_buf(),
            source,
        })?
    {
        return match resolve_directory_graph(
            request.apk_sha256,
            request.runtime_abi,
            request.elf_directory,
            request.cache_directory,
        )? {
            GraphSelection::CompleteDarwin(libraries) => Ok(ConversionOutcome::CompleteDarwin {
                publication: Publication::Existing,
                libraries,
            }),
            GraphSelection::AndroidElf(_) => Err(AdmissionError::Contract(
                "published Darwin cache directory is not a complete graph".to_owned(),
            )),
        };
    }

    let graph_sha256 = graph_sha256(&elf_paths)?;
    let converter_sha256 = match request.converter {
        Some(path) => executable_sha256(path)?,
        None => "none".to_owned(),
    };
    let marker = fallback_marker(request.cache_directory)?;
    let marker_contract = fallback_contract(
        request.apk_sha256,
        request.runtime_abi,
        &graph_sha256,
        &converter_sha256,
    );
    if fs::read_to_string(&marker).is_ok_and(|value| value == marker_contract) {
        return Ok(ConversionOutcome::AndroidElf {
            libraries: elf_paths,
            cached_failure: true,
            reason: "cached-incomplete-conversion",
        });
    }

    let Some(converter) = request.converter else {
        publish_fallback_marker(&marker, &marker_contract)?;
        return Ok(ConversionOutcome::AndroidElf {
            libraries: elf_paths,
            cached_failure: false,
            reason: "converter-unavailable",
        });
    };
    let parent = request.cache_directory.parent().ok_or_else(|| {
        AdmissionError::Contract("Darwin cache destination has no parent".to_owned())
    })?;
    fs::create_dir_all(parent).map_err(|source| AdmissionError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut stage = StageGuard::new(parent)?;
    let status = Command::new(converter)
        .arg("--apk-sha256")
        .arg(request.apk_sha256)
        .arg("--runtime-abi")
        .arg(request.runtime_abi)
        .arg("--elf-directory")
        .arg(request.elf_directory)
        .arg("--output-directory")
        .arg(stage.path())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|source| AdmissionError::Io {
            path: converter.to_path_buf(),
            source,
        })?;
    let admission = if status.success() {
        populate_contracts(request, stage.path(), &elf_paths)
    } else {
        Ok(())
    };
    if let Err(error) = &admission {
        eprintln!(
            "native-convert: rejected staged graph at {}: {error}",
            stage.path().display()
        );
    }
    if !status.success() || admission.is_err() {
        drop(stage);
        publish_fallback_marker(&marker, &marker_contract)?;
        return Ok(ConversionOutcome::AndroidElf {
            libraries: elf_paths,
            cached_failure: false,
            reason: if status.success() {
                "incomplete-conversion"
            } else {
                "converter-rejected-graph"
            },
        });
    }

    let publication = publish_complete_darwin_graph(
        request.apk_sha256,
        request.runtime_abi,
        request.elf_directory,
        stage.path(),
        request.cache_directory,
    )?;
    stage.disarm();
    if marker.exists() {
        let _ = fs::set_permissions(&marker, fs::Permissions::from_mode(0o600));
        let _ = fs::remove_file(&marker);
    }
    let libraries = match resolve_directory_graph(
        request.apk_sha256,
        request.runtime_abi,
        request.elf_directory,
        request.cache_directory,
    )? {
        GraphSelection::CompleteDarwin(libraries) => libraries,
        GraphSelection::AndroidElf(_) => {
            return Err(AdmissionError::Contract(
                "published conversion resolved back to Android ELF".to_owned(),
            ));
        }
    };
    Ok(ConversionOutcome::CompleteDarwin {
        publication,
        libraries,
    })
}

fn source_graph(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = direct_regular_files(directory, ".so")?;
    paths.sort();
    if paths.is_empty() {
        return Err(AdmissionError::Contract(
            "installed ELF directory contains no libraries".to_owned(),
        ));
    }
    for path in &paths {
        validate_android_elf(&fs::read(path).map_err(|source| AdmissionError::Io {
            path: path.clone(),
            source,
        })?)?;
    }
    Ok(paths)
}

fn graph_sha256(paths: &[PathBuf]) -> Result<String> {
    let mut digest = Sha256::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                AdmissionError::Contract("installed ELF SONAME is not UTF-8".to_owned())
            })?;
        let bytes = fs::read(path).map_err(|source| AdmissionError::Io {
            path: path.clone(),
            source,
        })?;
        digest.update((name.len() as u64).to_le_bytes());
        digest.update(name.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(&bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn executable_sha256(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path).map_err(|source| AdmissionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !path.is_absolute() || !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AdmissionError::Contract(
            "native converter must be an absolute regular non-symlink file".to_owned(),
        ));
    }
    let bytes = fs::read(path).map_err(|source| AdmissionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(sha256(&bytes))
}

fn populate_contracts(
    request: &ConversionRequest<'_>,
    stage: &Path,
    elf_paths: &[PathBuf],
) -> Result<()> {
    let mut expected = BTreeSet::new();
    for elf_path in elf_paths {
        let logical_soname = elf_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| AdmissionError::Contract("ELF SONAME is not UTF-8".to_owned()))?;
        let stem = logical_soname
            .strip_suffix(".so")
            .ok_or_else(|| AdmissionError::Contract("ELF source has no .so suffix".to_owned()))?;
        let dylib_name = format!("{stem}.dylib");
        expected.insert(dylib_name.clone());
        let dylib_path = stage.join(&dylib_name);
        let metadata = fs::symlink_metadata(&dylib_path).map_err(|source| AdmissionError::Io {
            path: dylib_path.clone(),
            source,
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(AdmissionError::Contract(format!(
                "converter output is not a regular file: {dylib_name}"
            )));
        }
        let elf = fs::read(elf_path).map_err(|source| AdmissionError::Io {
            path: elf_path.clone(),
            source,
        })?;
        let dylib = fs::read(&dylib_path).map_err(|source| AdmissionError::Io {
            path: dylib_path.clone(),
            source,
        })?;
        validate_complete_darwin_macho(&dylib)?;
        let contract = CompleteDarwinContract {
            logical_soname: logical_soname.to_owned(),
            apk_sha256: request.apk_sha256.to_owned(),
            source_elf_sha256: sha256(&elf),
            dylib_sha256: sha256(&dylib),
            runtime_abi: request.runtime_abi.to_owned(),
        };
        write_new(
            &stage.join(format!("{stem}.dylib.contract")),
            contract.encode().as_bytes(),
        )?;
    }
    let actual = fs::read_dir(stage)
        .map_err(|source| AdmissionError::Io {
            path: stage.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry
                .map_err(|source| AdmissionError::Io {
                    path: stage.to_path_buf(),
                    source,
                })?
                .file_name()
                .into_string()
                .map_err(|_| AdmissionError::Contract("converter filename is not UTF-8".to_owned()))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = expected
        .into_iter()
        .flat_map(|dylib| {
            let contract = format!("{dylib}.contract");
            [dylib, contract]
        })
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(AdmissionError::Contract(
            "converter output file set is not the exact native graph".to_owned(),
        ));
    }
    Ok(())
}

fn fallback_marker(cache: &Path) -> Result<PathBuf> {
    let name = cache
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AdmissionError::Contract("Darwin cache leaf is not UTF-8".to_owned()))?;
    Ok(cache.with_file_name(format!("{name}.elf-fallback")))
}

fn fallback_contract(apk: &str, abi: &str, graph: &str, converter: &str) -> String {
    format!(
        "{FALLBACK_VERSION}\napk-sha256={apk}\nruntime-abi={abi}\nelf-graph-sha256={graph}\nconverter-sha256={converter}\n"
    )
}

fn publish_fallback_marker(path: &Path, contract: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AdmissionError::Contract("fallback marker has no parent".to_owned()))?;
    fs::create_dir_all(parent).map_err(|source| AdmissionError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut temporary = None;
    for attempt in 0..128_u32 {
        let candidate = parent.join(format!(
            ".aim-fallback.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        match write_new(&candidate, contract.as_bytes()) {
            Ok(()) => {
                temporary = Some(candidate);
                break;
            }
            Err(AdmissionError::Io { source, .. })
                if source.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let temporary = temporary.ok_or_else(|| {
        AdmissionError::Contract("could not allocate fallback marker staging file".to_owned())
    })?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o400)).map_err(|source| {
        AdmissionError::Io {
            path: temporary.clone(),
            source,
        }
    })?;
    fs::rename(&temporary, path).map_err(|source| AdmissionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let _ = File::open(parent).and_then(|directory| directory.sync_all());
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| AdmissionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| AdmissionError::Io {
            path: path.to_path_buf(),
            source,
        })
}

struct StageGuard(Option<PathBuf>);

impl StageGuard {
    fn new(parent: &Path) -> Result<Self> {
        for attempt in 0..128_u32 {
            let path = parent.join(format!(
                ".aim-native-convert.{}.{}",
                std::process::id(),
                attempt
            ));
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => return Ok(Self(Some(path))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(AdmissionError::Io { path, source });
                }
            }
        }
        Err(AdmissionError::Contract(
            "could not allocate native conversion staging directory".to_owned(),
        ))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().expect("live conversion stage")
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            make_writable(path);
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn make_writable(path: &Path) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                make_writable(&child);
            } else {
                let _ = fs::set_permissions(child, fs::Permissions::from_mode(0o600));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CPU_TYPE_ARM64, CSMAGIC_EMBEDDED_SIGNATURE, ELF_MAGIC, LC_BUILD_VERSION, LC_CODE_SIGNATURE,
        LC_ID_DYLIB, MH_DYLIB, MH_MAGIC_64, PLATFORM_MACOS,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    const HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn unavailable_converter_fallback_is_graph_fingerprinted_and_cached() {
        let directory = TestDirectory::new();
        let elf_directory = directory.path.join("elf");
        fs::create_dir(&elf_directory).unwrap();
        fs::write(elf_directory.join("libfirst.so"), synthetic_elf()).unwrap();
        let cache = directory.path.join("cache/v1");
        let request = ConversionRequest {
            apk_sha256: HASH,
            runtime_abi: "v1",
            elf_directory: &elf_directory,
            cache_directory: &cache,
            converter: None,
        };
        assert!(matches!(
            prepare_complete_darwin_graph(&request).unwrap(),
            ConversionOutcome::AndroidElf {
                cached_failure: false,
                reason: "converter-unavailable",
                ..
            }
        ));
        assert!(matches!(
            prepare_complete_darwin_graph(&request).unwrap(),
            ConversionOutcome::AndroidElf {
                cached_failure: true,
                reason: "cached-incomplete-conversion",
                ..
            }
        ));
    }

    #[test]
    fn incomplete_converter_output_is_deleted_before_elf_fallback() {
        let directory = TestDirectory::new();
        let elf_directory = directory.path.join("elf");
        fs::create_dir(&elf_directory).unwrap();
        fs::write(elf_directory.join("libfirst.so"), synthetic_elf()).unwrap();
        let converter = directory.path.join("converter.sh");
        write_executable(&converter, "#!/bin/sh\nexit 0\n");
        let cache = directory.path.join("cache/v1");
        let outcome = prepare_complete_darwin_graph(&ConversionRequest {
            apk_sha256: HASH,
            runtime_abi: "v1",
            elf_directory: &elf_directory,
            cache_directory: &cache,
            converter: Some(&converter),
        })
        .unwrap();
        assert!(matches!(
            outcome,
            ConversionOutcome::AndroidElf {
                cached_failure: false,
                reason: "incomplete-conversion",
                ..
            }
        ));
        assert!(!cache.exists());
        assert!(fs::read_dir(cache.parent().unwrap()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("convert")
        }));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn complete_converter_graph_is_contracted_published_and_reused() {
        let directory = TestDirectory::new();
        let elf_directory = directory.path.join("elf");
        fs::create_dir(&elf_directory).unwrap();
        fs::write(elf_directory.join("libfirst.so"), synthetic_elf()).unwrap();
        let fixture = directory.path.join("fixture.dylib");
        fs::write(&fixture, synthetic_macho()).unwrap();
        let converter = directory.path.join("converter.sh");
        write_executable(
            &converter,
            &format!(
                "#!/bin/sh\nout=\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = --output-directory ]; then out=$2; shift 2; else shift 2; fi\ndone\ncp '{}' \"$out/libfirst.dylib\"\n",
                fixture.display()
            ),
        );
        let cache = directory.path.join("cache/v1");
        let request = ConversionRequest {
            apk_sha256: HASH,
            runtime_abi: "v1",
            elf_directory: &elf_directory,
            cache_directory: &cache,
            converter: Some(&converter),
        };
        assert!(matches!(
            prepare_complete_darwin_graph(&request).unwrap(),
            ConversionOutcome::CompleteDarwin {
                publication: Publication::Published,
                ..
            }
        ));
        assert!(cache.join("libfirst.dylib").is_file());
        assert!(
            CompleteDarwinContract::parse(
                &fs::read_to_string(cache.join("libfirst.dylib.contract")).unwrap()
            )
            .is_ok()
        );
        assert!(matches!(
            prepare_complete_darwin_graph(&request).unwrap(),
            ConversionOutcome::CompleteDarwin {
                publication: Publication::Existing,
                ..
            }
        ));
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn synthetic_elf() -> Vec<u8> {
        let mut elf = vec![0; 64];
        elf[..4].copy_from_slice(ELF_MAGIC);
        elf[4] = 2;
        elf[5] = 1;
        elf[18..20].copy_from_slice(&183_u16.to_le_bytes());
        elf
    }

    fn synthetic_macho() -> Vec<u8> {
        let id = "@loader_path/libfirst.dylib";
        let id_size = (24 + id.len() + 1 + 7) & !7;
        let command_bytes = id_size + 24 + 16;
        let signature_size = 12;
        let mut bytes = vec![0_u8; 32 + command_bytes + signature_size];
        bytes[0..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        bytes[4..8].copy_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
        bytes[12..16].copy_from_slice(&MH_DYLIB.to_le_bytes());
        bytes[16..20].copy_from_slice(&3_u32.to_le_bytes());
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
                "aim-native-conversion-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            make_writable(&self.path);
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

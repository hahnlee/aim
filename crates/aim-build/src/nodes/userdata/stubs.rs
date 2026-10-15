//! Source-derived compressed system payload requirements (#1269).
//! Original ScanPackageUtils sets STUB from compressedFileExists; init excludes
//! disabled-until-used, and installSystemStubPackages excludes disabled-user.
use aim_apps::{apk::Apk, res::Value};
use android_image_extract::{source::FileSource, zip::Archive};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_ENTRY: u64 = 512 << 20;
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    name: String,
    version: i64,
    split: bool,
}
/// Actual pre-boot Settings ownership, before fresh stub decompression disables
/// its factory record. An absent Settings file is a verified fresh input.
#[derive(Debug, Default)]
pub(super) struct Admission {
    saved_updates: BTreeMap<String, String>,
    settings_sha256: Option<String>,
}
impl Admission {
    pub(super) fn receipt(&self) -> serde_json::Value {
        serde_json::json!({"saved_updates": self.saved_updates, "settings_sha256": self.settings_sha256})
    }
    pub(super) fn read(volume: &Path) -> Result<Self, String> {
        let path = volume.join("data/system/packages.xml");
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        let root = aim_android_xml::read(&bytes)?;
        let mut disabled = BTreeMap::new();
        let mut active = BTreeMap::new();
        for node in root.children() {
            if !matches!(node.name.as_str(), "package" | "updated-package") {
                continue;
            }
            let name = node
                .string("name")
                .ok_or("pre-boot package identity absent")?
                .into_owned();
            if node.name == "updated-package" {
                let code = node
                    .string("codePath")
                    .ok_or("pre-boot disabled code path absent")?
                    .into_owned();
                if disabled.insert(name, code).is_some() {
                    return Err("duplicate pre-boot disabled owner".into());
                }
            } else if active
                .insert(
                    name,
                    node.string("codePath")
                        .ok_or("pre-boot code path absent")?
                        .into_owned(),
                )
                .is_some()
            {
                return Err("duplicate pre-boot active owner".into());
            }
        }
        let saved_updates = disabled
            .into_iter()
            .filter(|(name, _)| {
                active
                    .get(name)
                    .is_some_and(|path| path.starts_with("/data/app/"))
            })
            .collect();
        Ok(Self {
            saved_updates,
            settings_sha256: Some(format!("{:x}", Sha256::digest(&bytes))),
        })
    }
}
fn identity(apk: &Path) -> Result<Identity, String> {
    let parsed = Apk::open(apk).map_err(|e| e.to_string())?;
    let manifest = parsed.manifest().map_err(|e| e.to_string())?;
    let name = match manifest.named("package") {
        Some(Value::String(name)) if !name.is_empty() => name.clone(),
        _ => return Err(format!("{}: package identity missing", apk.display())),
    };
    let integer = |name| match manifest.named(name) {
        Some(Value::Int(value)) => Ok(*value as u32),
        None => Ok(0),
        _ => Err(format!("{}: nonliteral {name}", apk.display())),
    };
    let version =
        ((integer("versionCodeMajor")? as u64) << 32 | integer("versionCode")? as u64) as i64;
    let static_library = manifest
        .children
        .iter()
        .filter(|node| node.name == "application")
        .any(|application| {
            application
                .children
                .iter()
                .any(|node| node.name == "static-library")
        });
    let name = if static_library {
        format!("{name}_{version}")
    } else {
        name
    };
    let split = manifest.named("split").is_some();
    Ok(Identity {
        name,
        version,
        split,
    })
}
fn validate_zip(path: &Path) -> Result<(), String> {
    let source = FileSource::open(path).map_err(|e| e.to_string())?;
    let archive = Archive::open(&source).map_err(|e| e.to_string())?;
    let mut names = BTreeSet::new();
    for entry in &archive.entries {
        if !names.insert(entry.name.clone()) {
            return Err(format!("{}: duplicate ZIP entry", path.display()));
        }
        archive
            .read(entry, MAX_ENTRY)
            .map_err(|e| format!("{}: ZIP payload/CRC: {e}", path.display()))?;
    }
    Ok(())
}
fn sha(reader: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut count = 0u64;
    loop {
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        count += n as u64;
        if count > MAX_ENTRY {
            return Err("compressed artifact exceeds bound".into());
        }
        hash.update(&buffer[..n]);
    }
    Ok(hash.finalize().to_vec())
}
fn guest_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    if !path.starts_with('/')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err("unsafe captured package code path".into());
    }
    Ok(root.join(path.trim_start_matches('/')))
}
fn artifacts(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .ends_with(".gz")
        {
            if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
                return Err("compressed artifact is not a regular file".into());
            }
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}
fn source_stubs(image: &Path) -> Result<Vec<(PathBuf, Identity, Vec<PathBuf>)>, String> {
    fn walk(
        image: &Path,
        dir: &Path,
        output: &mut Vec<(PathBuf, Identity, Vec<PathBuf>)>,
    ) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if !kind.is_dir() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name();
            if let Some(base) = name.to_str().and_then(|n| n.strip_suffix("-Stub")) {
                let sibling = dir.join(base);
                if sibling.is_dir() {
                    let compressed = artifacts(&sibling)?;
                    if !compressed.is_empty() {
                        let mut bases = Vec::new();
                        for file in fs::read_dir(&path).map_err(|e| e.to_string())? {
                            let file = file.map_err(|e| e.to_string())?;
                            if file.path().extension().is_some_and(|ext| ext == "apk") {
                                let id = identity(&file.path())?;
                                if !id.split {
                                    bases.push(id);
                                }
                            }
                        }
                        if bases.len() != 1 {
                            return Err(format!(
                                "{}: compressed stub lacks unique base APK",
                                path.display()
                            ));
                        }
                        output.push((
                            path.strip_prefix(image).unwrap().into(),
                            bases.remove(0),
                            compressed,
                        ));
                    }
                }
            }
            walk(image, &path, output)?;
        }
        Ok(())
    }
    if !image.is_dir() {
        return Err("template image input absent".into());
    }
    let mut output = Vec::new();
    for partition in [
        "system",
        "system_ext",
        "product",
        "vendor",
        "odm",
        "oem",
        "apex",
    ] {
        let dir = image.join(partition);
        if dir.is_dir() {
            walk(image, &dir, &mut output)?;
        }
    }
    Ok(output)
}
fn installed_identity(
    volume: &Path,
    code: &str,
    name: &str,
    version: i64,
) -> Result<PathBuf, String> {
    let path = guest_path(volume, code)?;
    let files = if path.is_dir() {
        fs::read_dir(&path)
            .map_err(|e| e.to_string())?
            .map(|entry| entry.map(|entry| entry.path()).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![path.clone()]
    };
    let mut bases = 0;
    for file in files
        .into_iter()
        .filter(|file| file.extension().is_some_and(|ext| ext == "apk"))
    {
        validate_zip(&file)?;
        let id = identity(&file)?;
        if id.name != name || id.version != version {
            return Err(format!("{name}: installed APK identity differs"));
        }
        if !id.split {
            bases += 1;
        }
    }
    if bases != 1 {
        return Err(format!(
            "{name}: required installed base APK missing or ambiguous"
        ));
    }
    Ok(path)
}
pub(super) fn validate(
    image: &Path,
    volume: &Path,
    captured: &Path,
    packages: &aim_android_xml::Element,
    admission: &Admission,
) -> Result<(), String> {
    let mut active = BTreeMap::new();
    let mut disabled = BTreeMap::new();
    for node in packages.children() {
        if !matches!(node.name.as_str(), "package" | "updated-package") {
            continue;
        }
        let name = node
            .string("name")
            .ok_or("package identity absent")?
            .into_owned();
        let map = if node.name == "package" {
            &mut active
        } else {
            &mut disabled
        };
        if map.insert(name, node).is_some() {
            return Err("duplicate captured package identity".into());
        }
    }
    let user_path = captured.join("data/system/users/0/package-restrictions.xml");
    let user = aim_android_xml::read(
        &fs::read(&user_path).map_err(|e| format!("{}: {e}", user_path.display()))?,
    )?;
    let mut states = BTreeMap::new();
    for node in user.children().filter(|node| node.name == "pkg") {
        let name = node
            .string("name")
            .ok_or("user package identity absent")?
            .into_owned();
        if states
            .insert(name, node.int("enabled")?.unwrap_or(0))
            .is_some()
        {
            return Err("duplicate user package state".into());
        }
    }
    for (stub_path, id, compressed) in source_stubs(image)? {
        let setting = active
            .get(&id.name)
            .ok_or_else(|| format!("{}: image stub has no captured admission owner", id.name))?;
        let code = setting
            .string("codePath")
            .ok_or("active code path absent")?;
        let version = setting.long("version")?.unwrap_or(0);
        if code.starts_with("/data/app/") {
            let factory = disabled.get(&id.name).ok_or_else(|| {
                format!(
                    "{}: data update lacks original disabled-system owner",
                    id.name
                )
            })?;
            let factory_code = factory
                .string("codePath")
                .ok_or("disabled-system code path absent")?;
            if Path::new(factory_code.trim_start_matches('/')) != stub_path
                || factory.long("version")?.unwrap_or(0) != id.version
            {
                return Err(format!(
                    "{}: disabled factory identity differs from image stub",
                    id.name
                ));
            }
            let actual = installed_identity(volume, &code, &id.name, version)?;
            // A genuine saved/newer data update is an original exclusion from
            // decompression; its real package identity and ZIP CRCs remain mandatory.
            if let Some(saved_factory) = admission.saved_updates.get(&id.name) {
                if saved_factory != &*factory_code {
                    return Err(format!("{}: saved factory owner changed", id.name));
                }
                continue;
            }
            if version != id.version {
                return Err(format!(
                    "{}: fresh payload version differs from image stub",
                    id.name
                ));
            }
            if !actual.is_dir() {
                return Err(format!(
                    "{}: fresh decompressed payload is not a directory",
                    id.name
                ));
            }
            for source in compressed {
                let filename = source
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("compressed artifact name is not UTF8")?;
                let filename = &filename[..filename.len() - 3];
                let target = actual.join(filename);
                let mut input = flate2::read::MultiGzDecoder::new(
                    fs::File::open(&source).map_err(|e| e.to_string())?,
                );
                let expected = sha(&mut input)
                    .map_err(|e| format!("{}: gzip payload/CRC: {e}", source.display()))?;
                let mut output = fs::File::open(&target)
                    .map_err(|e| format!("{}: required artifact absent: {e}", target.display()))?;
                if sha(&mut output)? != expected {
                    return Err(format!(
                        "{}: decompressed payload differs",
                        target.display()
                    ));
                }
            }
        } else if states
            .get(&id.name)
            .is_some_and(|state| matches!(state, 3 | 4))
        {
            continue;
        } else if Path::new(code.trim_start_matches('/')) == stub_path {
            return Err(format!(
                "{}: mandatory admitted compressed payload not installed",
                id.name
            ));
        } else {
            return Err(format!(
                "{}: captured code does not bind image stub or valid data update",
                id.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, process::Command};
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn xml(text: &str) -> aim_android_xml::Element {
        aim_android_xml::read(text.as_bytes()).unwrap()
    }
    #[test]
    fn genuine_apk_payload_admission_crc_and_saved_update() {
        let sdk = aim_paths::sdk().expect("NOT RUN: Android SDK unavailable");
        let aapt = sdk.join("build-tools/36.0.0/aapt2");
        let android = sdk.join("platforms/android-36/android.jar");
        assert!(
            aapt.is_file() && android.is_file(),
            "NOT RUN: pinned SDK36 inputs unavailable"
        );
        let temp =
            Temp(std::env::temp_dir().join(format!("aim-stub-validator-{}", std::process::id())));
        fs::create_dir(&temp.0).unwrap();
        let image = temp.0.join("image");
        let volume = temp.0.join("volume");
        let capture = temp.0.join("capture");
        let stub = image.join("product/app/Payload-Stub");
        let compressed = image.join("product/app/Payload");
        let data = volume.join("data/app/unique/Payload");
        for path in [
            &stub,
            &compressed,
            &data,
            &capture.join("data/system/users/0"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        let manifest = temp.0.join("AndroidManifest.xml");
        fs::write(&manifest, r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="test.payload" android:versionCode="7"><application android:hasCode="false"/></manifest>"#).unwrap();
        let apk = stub.join("Payload.apk");
        let output = Command::new(&aapt)
            .args(["link", "--manifest"])
            .arg(&manifest)
            .arg("-I")
            .arg(&android)
            .arg("-o")
            .arg(&apk)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes = fs::read(&apk).unwrap();
        let gzip = compressed.join("Payload.apk.gz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        let gzip_bytes = encoder.finish().unwrap();
        fs::write(&gzip, &gzip_bytes).unwrap();
        let user = capture.join("data/system/users/0/package-restrictions.xml");
        fs::write(&user, "<package-restrictions><pkg name=\"test.payload\" enabled=\"2\"/></package-restrictions>").unwrap();
        let failed = xml("<packages><package name=\"test.payload\" codePath=\"/product/app/Payload-Stub\" version=\"7\"/></packages>");
        assert!(
            validate(&image, &volume, &capture, &failed, &Admission::default())
                .unwrap_err()
                .contains("not installed")
        );
        fs::write(&user, "<package-restrictions><pkg name=\"test.payload\" enabled=\"3\"/></package-restrictions>").unwrap();
        validate(&image, &volume, &capture, &failed, &Admission::default()).unwrap();
        let installed = xml("<packages><package name=\"test.payload\" codePath=\"/data/app/unique/Payload\" version=\"7\"/><updated-package name=\"test.payload\" codePath=\"/product/app/Payload-Stub\" version=\"7\"/></packages>");
        assert!(
            validate(&image, &volume, &capture, &installed, &Admission::default())
                .unwrap_err()
                .contains("missing")
        );
        fs::write(data.join("Payload.apk"), &bytes).unwrap();
        validate(&image, &volume, &capture, &installed, &Admission::default()).unwrap();
        let mut concatenated = Vec::new();
        for half in bytes.chunks(bytes.len().div_ceil(2)) {
            let mut stream =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            stream.write_all(half).unwrap();
            concatenated.extend(stream.finish().unwrap());
        }
        fs::write(&gzip, concatenated).unwrap();
        validate(&image, &volume, &capture, &installed, &Admission::default()).unwrap();
        fs::write(&gzip, &gzip_bytes[..gzip_bytes.len() - 4]).unwrap();
        assert!(
            validate(&image, &volume, &capture, &installed, &Admission::default())
                .unwrap_err()
                .contains("gzip")
        );
        fs::create_dir_all(volume.join("data/system")).unwrap();
        fs::write(volume.join("data/system/packages.xml"), "<packages><package name=\"test.payload\" codePath=\"/data/app/unique/Payload\"/><updated-package name=\"test.payload\" codePath=\"/product/app/Payload-Stub\"/></packages>").unwrap();
        let saved = Admission::read(&volume).unwrap();
        // A real independently built same-version APK differs bytewise while
        // retaining the original manifest identity and valid ZIP CRCs.
        fs::write(&manifest, r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="test.payload" android:versionCode="7"><application android:hasCode="false" android:label="Updated payload"/></manifest>"#).unwrap();
        let updated = data.join("Payload.apk");
        let output = Command::new(&aapt)
            .args(["link", "--manifest"])
            .arg(&manifest)
            .arg("-I")
            .arg(&android)
            .arg("-o")
            .arg(&updated)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_ne!(fs::read(&updated).unwrap(), bytes);
        fs::write(&gzip, &gzip_bytes).unwrap();
        assert!(
            validate(&image, &volume, &capture, &installed, &Admission::default())
                .unwrap_err()
                .contains("payload differs")
        );
        validate(&image, &volume, &capture, &installed, &saved).unwrap();
        fs::write(data.join("Payload.apk"), b"bad ZIP").unwrap();
        assert!(validate(&image, &volume, &capture, &installed, &saved).is_err());
        fs::remove_file(volume.join("data/system/packages.xml")).unwrap();
        assert!(Admission::read(&volume).unwrap().saved_updates.is_empty());
        fs::create_dir(volume.join("data/system/packages.xml")).unwrap();
        assert!(Admission::read(&volume).is_err());
        assert!(validate(
            &image,
            &volume,
            &capture,
            &xml("<packages/>"),
            &Admission::default()
        )
        .is_err());
    }
}

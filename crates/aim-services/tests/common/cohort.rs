//! Explicit pinned inputs for the M4 owner gates; no public artifact overrides.
//! The source row binds the actual workspace root/HEAD, not compiled code.
//! The runner must record fixture source/blob and binary hashes at compilation,
//! check the source before/after execution, and cross-check every manifest pin
//! against its approved runtime/image/userdata receipt before publishing inputs.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}, process::Command};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Variant { Original, Native }
impl Variant {
    fn parse(value: &str) -> Result<Self, String> {
        match value { "original" => Ok(Self::Original), "native" => Ok(Self::Native),
            _ => Err(format!("invalid image variant {value}")) }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
struct Stamp { dev: u64, ino: u64, size: u64, modified: (i64,i64), changed: (i64,i64) }
fn stamp(path: &Path) -> Result<Stamp,String> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(path).map_err(|e| format!("{}: {e}",path.display()))?;
    Ok(Stamp {dev:m.dev(),ino:m.ino(),size:m.len(),modified:(m.mtime(),m.mtime_nsec()),changed:(m.ctime(),m.ctime_nsec())})
}
#[derive(Clone, Debug)]
pub struct Pin { pub path: PathBuf, pub sha256: String }
impl Pin {
    fn parse(path: &str, hash: &str) -> Result<Self, String> {
        if !Path::new(path).is_absolute() || !digest_valid(hash) { return Err("invalid absolute path or SHA256 pin".into()); }
        Ok(Self { path: path.into(), sha256: hash.into() })
    }
    fn verify(&self) -> Result<Stamp, String> {
        let before = stamp(&self.path)?;
        if sha(&self.path)? != self.sha256 || stamp(&self.path)? != before { return Err(format!("stale input SHA256/binding: {}", self.path.display())); }
        Ok(before)
    }
}
#[derive(Clone, Debug)]
pub struct Image { pub root: PathBuf, pub receipt: Pin, pub files: BTreeMap<String, Pin> }
#[derive(Clone, Debug)]
pub struct Cohort {
    pub source_root: PathBuf, pub source_head: String, pub runtime_receipt: Pin,
    pub runtime: BTreeMap<String, Pin>, pub images: BTreeMap<Variant, Image>,
    pub tools: BTreeMap<String, Pin>, pub userdata: PathBuf, pub userdata_sha256: String,
    pub userdata_receipt: Pin,
    manifest: Pin,
    bindings: BTreeMap<PathBuf,Stamp>,
    userdata_bindings: BTreeMap<PathBuf,Stamp>,
}
const BINARIES: [&str; 5] = ["linux-run", "guest-init", "aim-lock-holder", "aim-binderd", "aim-display"];
const IMAGE_FILES: [(&str, &str); 4] = [
    ("services", "system/framework/services.jar"), ("java", "system/framework/aim-services.jar"),
    ("native_services", "system/etc/aim/native-services"),
    ("namespace_policy", "vendor/etc/aim/init-mount-namespace-policy.conf"),
];
const TOOLS: [&str; 9] = ["aimctl", "aim-apps", "java", "javac", "aidl", "d8", "aapt2", "apksigner", "ndk_clang35"];
fn digest_valid(value: &str) -> bool { value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn sha(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err(format!("not a pinned file: {}", path.display())); }
    let mut digest = Sha256::new(); let mut bytes = [0; 65536];
    loop { let n = file.read(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?; if n == 0 { break; } digest.update(&bytes[..n]); }
    Ok(format!("{:x}", digest.finalize()))
}
/// Directory digest: sorted UTF8 relative paths, NUL, file SHA256, newline.
/// Symbolic links and special files are rejected rather than following unpinned inputs.
pub fn tree_sha(root: &Path) -> Result<String, String> {
    fn walk(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?; let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_dir() { walk(root, &entry.path(), files)?; }
            else if kind.is_file() {
                let path = entry.path(); let name = path.strip_prefix(root).unwrap().to_str().ok_or("nonUTF8 input path")?.to_string();
                files.push((name, path));
            } else { return Err("userdata contains unpinned symlink or special file".into()); }
        }
        Ok(())
    }
    let mut files = Vec::new(); walk(root, root, &mut files)?;
    if files.is_empty() { return Err("userdata input directory is empty".into()); }
    files.sort_by(|a,b| a.0.cmp(&b.0)); let mut digest = Sha256::new();
    for (name, path) in files { digest.update(name.as_bytes()); digest.update([0]); digest.update(sha(&path)?.as_bytes()); digest.update(b"\n"); }
    Ok(format!("{:x}", digest.finalize()))
}
fn track(bindings:&mut BTreeMap<PathBuf,Stamp>,pin:&Pin)->Result<(),String>{
    bindings.insert(pin.path.clone(),pin.verify()?); Ok(())
}
fn tree_bindings(root:&Path)->Result<BTreeMap<PathBuf,Stamp>,String>{
    fn walk(path:&Path,out:&mut BTreeMap<PathBuf,Stamp>)->Result<(),String>{
        let metadata=fs::symlink_metadata(path).map_err(|e|e.to_string())?;
        if metadata.file_type().is_symlink()||(!metadata.is_file()&&!metadata.is_dir()){return Err("unsupported userdata binding".into());}
        out.insert(path.into(),stamp(path)?);
        if metadata.is_dir(){for entry in fs::read_dir(path).map_err(|e|e.to_string())?{walk(&entry.map_err(|e|e.to_string())?.path(),out)?;}}
        Ok(())
    }
    let mut out=BTreeMap::new();walk(root,&mut out)?;Ok(out)
}
fn insert<T>(map: &mut BTreeMap<String, T>, key: &str, value: T) -> Result<(), String> {
    if map.insert(key.into(), value).is_some() { return Err(format!("duplicate input {key}")); } Ok(())
}
impl Cohort {
    /// Strict TSV v1. Unknown records, duplicates and absent pins are errors.
    pub fn read(path: &Path, expected_root: &Path, expected_head: &str) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("NOT RUN: cohort input {}: {e}", path.display()))?;
        let parse = || -> Result<Self, String> {
            if text.len() > 65536 { return Err("cohort input exceeds64KiB".into()); }
            let mut bindings=BTreeMap::new();
            let mut singles: BTreeMap<String, Vec<String>> = BTreeMap::new(); let mut runtime = BTreeMap::new(); let mut tools = BTreeMap::new();
            let mut roots = BTreeMap::new(); let mut receipts = BTreeMap::new(); let mut image_files: BTreeMap<Variant, BTreeMap<String, Pin>> = BTreeMap::new();
            for line in text.lines() {
                if line.is_empty() || line.starts_with('#') { continue; }
                match line.split('\t').collect::<Vec<_>>().as_slice() {
                    ["schema", "1"] => insert(&mut singles, "schema", vec!["1".into()])?,
                    ["source", root, head] => insert(&mut singles, "source", vec![(*root).into(), (*head).into()])?,
                    ["runtime_receipt", p, h] => insert(&mut singles, "runtime_receipt", vec![(*p).into(), (*h).into()])?,
                    ["userdata", p, h] => insert(&mut singles, "userdata", vec![(*p).into(), (*h).into()])?,
                    ["userdata_receipt", p, h] => insert(&mut singles, "userdata_receipt", vec![(*p).into(), (*h).into()])?,
                    ["runtime", name, p, h] if BINARIES.contains(name) => insert(&mut runtime, name, Pin::parse(p,h)?)?,
                    ["tool", name, p, h] if TOOLS.contains(name) => insert(&mut tools, name, Pin::parse(p,h)?)?,
                    ["image", variant, p] => {
                        let variant = Variant::parse(variant)?;
                        if roots.insert(variant, PathBuf::from(p)).is_some() { return Err("duplicate image root".into()); }
                    }
                    ["image_receipt", variant, p, h] => {
                        if receipts.insert(Variant::parse(variant)?, Pin::parse(p,h)?).is_some() { return Err("duplicate image receipt".into()); }
                    }
                    ["image_file", variant, name, p, h] if IMAGE_FILES.iter().any(|(label,_)| label == name) => {
                        insert(image_files.entry(Variant::parse(variant)?).or_default(), name, Pin::parse(p,h)?)?;
                    }
                    _ => return Err("unknown or malformed cohort record".into()),
                }
            }
            let take = |name: &str| singles.get(name).ok_or_else(|| format!("missing input {name}"));
            take("schema")?; let source = take("source")?;
            if Path::new(&source[0]) != expected_root || source[1] != expected_head { return Err("stale workspace root/head binding".into()); }
            let getpin = |name: &str| { let fields = take(name)?; Pin::parse(&fields[0],&fields[1]) };
            let runtime_receipt = getpin("runtime_receipt")?; track(&mut bindings,&runtime_receipt)?;
            for name in BINARIES { track(&mut bindings,runtime.get(name).ok_or_else(|| format!("missing runtime {name}"))?)?; }
            let parent = runtime["linux-run"].path.parent().ok_or("runtime parent missing")?;
            for name in BINARIES {
                if runtime[name].path.parent() != Some(parent) || runtime[name].path.file_name().and_then(|n|n.to_str()) != Some(name) { return Err("mixed runtime sibling cohort".into()); }
            }
            for name in TOOLS { track(&mut bindings,tools.get(name).ok_or_else(|| format!("missing tool {name}"))?)?; }
            let ctl = &tools["aimctl"].path; let apps = &tools["aim-apps"].path;
            if ctl.parent() != apps.parent()
                || fs::canonicalize(ctl).map_err(|e|e.to_string())?.parent() != fs::canonicalize(apps).map_err(|e|e.to_string())?.parent()
                || apps.file_name().and_then(|name|name.to_str()) != Some("aim-apps") {
                return Err("controller window helper is not the pinned aimctl sibling".into());
            }
            let mut images = BTreeMap::new();
            for variant in [Variant::Original, Variant::Native] {
                let root = roots.remove(&variant).ok_or("missing image variant")?;
                if !root.is_absolute() || !root.is_dir() { return Err("missing absolute image root".into()); }
                let receipt = receipts.remove(&variant).ok_or("missing image receipt")?; track(&mut bindings,&receipt)?;
                let files = image_files.remove(&variant).ok_or("missing image file pins")?;
                for (name, relative) in IMAGE_FILES {
                    let file = files.get(name).ok_or("missing image Jar/policy pin")?;
                    if file.path != root.join(relative) { return Err("image pin refers to another root".into()); }
                    track(&mut bindings,file)?;
                }
                let declarations = fs::read_to_string(&files["native_services"].path).map_err(|e| e.to_string())?;
                let mut services = BTreeMap::new();
                for service in aim_android_image::system_server::parse_native_services(&declarations)
                    .map_err(|error|format!("malformed native service declaration: {error}"))? {
                    if services.insert(service.name,service.class).is_some() { return Err("duplicate native service declaration".into()); }
                }
                if variant == Variant::Original && services.contains_key("package")
                    || variant == Variant::Native && services.get("package").map(String::as_str) != Some("com.android.server.pm.PackageManagerService") {
                    return Err("image variant does not match package service activation".into());
                }
                images.insert(variant, Image {root, receipt, files});
            }
            let fields = take("userdata")?; let userdata = PathBuf::from(&fields[0]);
            let userdata_bindings=tree_bindings(&userdata)?;
            if !userdata.is_absolute() || !digest_valid(&fields[1]) || tree_sha(&userdata)? != fields[1] || tree_bindings(&userdata)? != userdata_bindings { return Err("missing or stale userdata tree pin".into()); }
            let userdata_receipt = getpin("userdata_receipt")?; track(&mut bindings,&userdata_receipt)?;
            let manifest=Pin::parse(path.to_str().ok_or("nonUTF8 manifest path")?,&format!("{:x}",Sha256::digest(text.as_bytes())))?;
            track(&mut bindings,&manifest)?;
            Ok(Self {manifest,bindings,userdata_bindings,source_root:source[0].clone().into(),source_head:source[1].clone(),runtime_receipt,runtime,images,tools,userdata,userdata_sha256:fields[1].clone(),userdata_receipt})
        };
        parse().map_err(|e| format!("NOT RUN: invalid cohort inputs: {e}"))
    }
    pub fn revalidate(&self, launch: bool) -> Result<(),String> {
        let check=||->Result<(),String>{
            if sha(&self.manifest.path)? != self.manifest.sha256 {return Err("cohort manifest changed".into());}
            for (path,before) in &self.bindings {if stamp(path)? != *before {return Err(format!("pinned input binding changed: {}",path.display()));}}
            if tree_bindings(&self.userdata)? != self.userdata_bindings {return Err("userdata binding changed".into());}
            if launch && tree_sha(&self.userdata)? != self.userdata_sha256 {return Err("userdata tree digest changed".into());}
            Ok(())
        };check().map_err(|e|format!("NOT RUN: stale cohort: {e}"))
    }
    pub fn verify_controller(&self) -> Result<(),String> { self.tools["aimctl"].verify().map(|_|()) }
    pub fn runtime_root(&self) -> &Path { self.runtime["linux-run"].path.parent().unwrap() }
    pub fn image(&self, variant: Variant) -> &Path { &self.images[&variant].root }
}
pub fn load() -> Result<Cohort, String> {
    let root = aim_paths::root();
    let output = Command::new("git").args(["-C"]).arg(root).args(["rev-parse", "HEAD"]).output().map_err(|e| format!("NOT RUN: source binding: {e}"))?;
    if !output.status.success() { return Err("NOT RUN: cannot identify fixture source HEAD".into()); }
    Cohort::read(&aim_paths::out().join("m4-integration.inputs"), root, String::from_utf8_lossy(&output.stdout).trim())
}
pub fn original_image() -> PathBuf { load().expect("NOT RUN: explicit M4 cohort required").image(Variant::Original).into() }

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture { root: PathBuf, text: String }
    impl Drop for Fixture { fn drop(&mut self) { fs::remove_dir_all(&self.root).unwrap(); } }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!("aim-cohort-pins-{}-{}", std::process::id(),
                NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
            fs::create_dir(&root).unwrap(); let root = fs::canonicalize(root).unwrap();
            let make = |path: &Path, bytes: &[u8]| {
                fs::create_dir_all(path.parent().unwrap()).unwrap(); fs::write(path, bytes).unwrap(); sha(path).unwrap()
            };
            let mut text = format!("schema\t1\nsource\t{}\t{}\n",root.display(), "1".repeat(40));
            let receipt = root.join("runtime.receipt"); let hash = make(&receipt,b"authored digest fixture");
            text += &format!("runtime_receipt\t{}\t{hash}\n",receipt.display());
            for name in BINARIES { let path = root.join("runtime").join(name); let hash = make(&path,name.as_bytes()); text += &format!("runtime\t{name}\t{}\t{hash}\n",path.display()); }
            for name in TOOLS { let path = root.join("tools").join(name); let hash = make(&path,name.as_bytes()); text += &format!("tool\t{name}\t{}\t{hash}\n",path.display()); }
            for (variant,name) in [(Variant::Original,"original"),(Variant::Native,"native")] {
                let image = root.join(name); fs::create_dir(&image).unwrap(); text += &format!("image\t{name}\t{}\n",image.display());
                let path = root.join(format!("{name}.receipt")); let hash = make(&path,name.as_bytes()); text += &format!("image_receipt\t{name}\t{}\t{hash}\n",path.display());
                for (label,relative) in IMAGE_FILES {
                    let path = image.join(relative); let bytes = if label == "native_services" {
                        if variant == Variant::Native {b"package com.android.server.pm.PackageManagerService\n".as_slice()} else {b"clipboard com.android.server.clipboard.ClipboardService\n".as_slice()}
                    } else {relative.as_bytes()};
                    let hash = make(&path,bytes); text += &format!("image_file\t{name}\t{label}\t{}\t{hash}\n",path.display());
                }
            }
            let userdata = root.join("userdata"); make(&userdata.join("template.asif"),b"authored template bytes");
            text += &format!("userdata\t{}\t{}\n",userdata.display(),tree_sha(&userdata).unwrap());
            let receipt = root.join("userdata.receipt"); let hash = make(&receipt,b"authored capture receipt"); text += &format!("userdata_receipt\t{}\t{hash}\n",receipt.display());
            Self {root,text}
        }
        fn read(&self) -> Result<Cohort,String> {
            let path = self.root.join("inputs"); fs::write(&path,&self.text).unwrap(); Cohort::read(&path,&self.root,&"1".repeat(40))
        }
    }
    #[test]
    fn explicit_pins_select_both_images_and_reject_stale_or_mixed_inputs() {
        let fixture = Fixture::new(); let checked = fixture.read().unwrap();
        assert_eq!(checked.image(Variant::Original),fixture.root.join("original"));
        assert_eq!(checked.image(Variant::Native),fixture.root.join("native"));
        assert_eq!(checked.runtime_root(),fixture.root.join("runtime"));
        checked.revalidate(false).unwrap(); checked.revalidate(true).unwrap();
        fs::write(&checked.runtime["linux-run"].path,b"changed after pin").unwrap();
        assert!(checked.revalidate(false).unwrap_err().contains("binding changed"));
        assert!(fixture.read().unwrap_err().contains("stale input SHA256"));
        let mut fixture = Fixture::new();
        let duplicate = fixture.text.lines().find(|l|l.starts_with("runtime\tlinux-run\t")).unwrap().to_owned();
        fixture.text += &format!("{duplicate}\n"); assert!(fixture.read().unwrap_err().contains("duplicate"));
        let mut fixture = Fixture::new();
        let old = fixture.root.join("runtime/aim-display");
        let new = fixture.root.join("other-runtime/aim-display");
        fs::create_dir_all(new.parent().unwrap()).unwrap(); fs::copy(&old,&new).unwrap();
        fixture.text = fixture.text.replace(old.to_str().unwrap(),new.to_str().unwrap());
        assert!(fixture.read().unwrap_err().contains("mixed runtime sibling"));
        let mut fixture = Fixture::new(); fixture.text = fixture.text.lines().filter(|l|!l.starts_with("tool\taidl\t")).collect::<Vec<_>>().join("\n");
        assert!(fixture.read().unwrap_err().contains("missing tool aidl"));
        let mut fixture = Fixture::new();
        let old = fixture.root.join("tools/aim-apps");
        let new = fixture.root.join("other-tools/aim-apps");
        fs::create_dir_all(new.parent().unwrap()).unwrap(); fs::copy(&old,&new).unwrap();
        fixture.text = fixture.text.replace(old.to_str().unwrap(),new.to_str().unwrap());
        assert!(fixture.read().unwrap_err().contains("pinned aimctl sibling"));
        let fixture = Fixture::new(); let path = fixture.root.join("inputs"); fs::write(&path,&fixture.text).unwrap();
        assert!(Cohort::read(&path,&fixture.root,&"2".repeat(40)).unwrap_err().contains("workspace root/head"));
    }
    #[test]
    fn native_service_schema_rejects_malformed_and_duplicate_declarations() {
        for declarations in [b"package\n".as_slice(), b"package com.android.server.pm.PackageManagerService\npackage com.android.server.pm.PackageManagerService\n".as_slice()] {
            let mut fixture = Fixture::new();
            let path = fixture.root.join("native/system/etc/aim/native-services");
            let old = sha(&path).unwrap(); fs::write(&path,declarations).unwrap();
            fixture.text = fixture.text.replace(&old,&sha(&path).unwrap());
            let error = fixture.read().unwrap_err();
            assert!(error.contains("malformed native service") || error.contains("duplicate native service"), "{error}");
        }
    }
    #[test]
    fn userdata_drift_and_wrong_activation_are_errors_not_successful_skips() {
        let fixture = Fixture::new(); let retained = fixture.read().unwrap();
        fs::write(fixture.root.join("userdata/extra.asif"),b"unrecorded").unwrap();
        assert!(retained.revalidate(false).unwrap_err().contains("userdata binding"));
        assert!(fixture.read().unwrap_err().contains("userdata tree pin"));
        let mut fixture = Fixture::new(); let path = fixture.root.join("original/system/etc/aim/native-services");
        let old = sha(&path).unwrap(); fs::write(&path,b"package com.android.server.pm.PackageManagerService\n").unwrap(); let new = sha(&path).unwrap(); fixture.text = fixture.text.replace(&old,&new);
        assert!(fixture.read().unwrap_err().contains("service activation"));
        let fixture = Fixture::new();
        assert!(Cohort::read(&fixture.root.join("missing"),&fixture.root,&"1".repeat(40)).unwrap_err().starts_with("NOT RUN:"));
    }
}

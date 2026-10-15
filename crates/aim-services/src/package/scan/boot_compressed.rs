//! InstallPackageHelper.decompressPackage, android-16.0.0_r1 (Apache-2.0).
//! A release receipt proves gzip and native-library work completed on writable data.
use super::{AbiPolicy, NativeLibraryDestination, NativeLibraryInstallPolicy, NativeLibraryPackageCopy};
use crate::package::{pkg::{AndroidPackage, booleans, booleans2}, write::Apks};
use aim_storage::guest_inode::{GuestInode, record};
use std::{fs::{self, File, OpenOptions}, io::{self, Read, Write}, os::unix::fs::{MetadataExt, PermissionsExt}, path::{Path, PathBuf}, sync::Mutex, time::SystemTime};

#[derive(Clone, Debug)]
pub struct Receipt { pub guest_path: String, host_path: PathBuf, device: u64, inode: u64 }
pub struct Expanded { pub package: AndroidPackage, pub libraries: NativeLibraryPackageCopy, pub receipt: Receipt }
/// Construct from the guest filesystem's writable /data/app mapping, separately
/// from Apks.files, whose read mapping includes immutable image paths.
pub struct BootDecompression { data_app: PathBuf, pending: Mutex<Vec<Receipt>>, release_gate: Mutex<()> }
impl BootDecompression {
    pub fn new(data_app: PathBuf) -> Result<Self, String> {
        let metadata = fs::symlink_metadata(&data_app).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() { return Err("Writable /data/app owner is not a resolved directory".into()) }
        Ok(Self { data_app, pending: Mutex::new(vec![]), release_gate: Mutex::new(()) })
    }
    /// Match installSystemStubPackages admission. Caller iterates its stub list
    /// backwards; skipped entries are removed without touching enabled state.
    pub fn admitted(disabled_factory: bool, loaded: Option<&AndroidPackage>, system_user_enabled: i32) -> bool {
        !disabled_factory && loaded.is_some() && system_user_enabled != 3
    }
    pub fn decompress(&self, stub: &AndroidPackage, apks: &Apks, abis: &AbiPolicy,
        policy: NativeLibraryInstallPolicy, zip_time: &dyn Fn(u32) -> Result<SystemTime, String>,
        restorecon: &dyn Fn(&Path) -> Result<(), String>) -> Result<Option<Expanded>, String> {
        if !stub.is2(booleans2::STUB) { return Err("Boot decompression requires actual stub code".into()) }
        let code = Path::new(stub.path.as_deref().ok_or("Stub code path missing")?);
        let Some(name) = code.file_name().and_then(|v| v.to_str()).and_then(|v| v.strip_suffix("-Stub")) else { return Ok(None) };
        let sibling = code.parent().ok_or("Stub code parent missing")?.join(name);
        let source = (apks.files)(sibling.to_str().ok_or("Compressed sibling path is not UTF-8")?).ok_or("Compressed sibling mapping missing")?;
        let entries = match fs::read_dir(&source) { Ok(entries) => entries, Err(e) if e.kind() == io::ErrorKind::NotFound || e.kind() == io::ErrorKind::NotADirectory => return Ok(None), Err(e) => return Err(e.to_string()) };
        let mut compressed = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name().into_string().map_err(|_| "Compressed artifact name is not UTF-8")?;
            if name.to_lowercase().ends_with(".gz") { compressed.push((name, entry.path())); }
        }
        if compressed.is_empty() { return Ok(None) }
        let (guest, host, parent) = self.allocate(&stub.package_name)?;
        let result = (|| -> Result<Expanded, String> {
            fs::create_dir(&host).map_err(|e| e.to_string())?;
            directory_metadata(&host)?;
            for (name, source) in compressed {
                let output_name = &name[..name.len() - 3];
                if output_name.is_empty() { return Err("Compressed output name is empty".into()) }
                let temporary = host.join(format!("{output_name}.new"));
                let mut output = OpenOptions::new().create_new(true).write(true).open(&temporary).map_err(|e| e.to_string())?;
                let mut input = flate2::read::MultiGzDecoder::new(File::open(source).map_err(|e| e.to_string())?);
                io::copy(&mut input, &mut output).map_err(|e| e.to_string())?;
                output.flush().map_err(|e| e.to_string())?;
                fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
                record(&temporary, GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o644) }).map_err(|e| e.to_string())?;
                output.sync_all().map_err(|e| e.to_string())?;
                fs::rename(&temporary, host.join(output_name)).map_err(|e| e.to_string())?;
            }
            let package = apks.parsed_path(&guest, 0)?;
            // Handle.create reads the resulting APK manifest, including native
            // extraction flags; no ABI selection from the stub's empty payload.
            let policy = NativeLibraryInstallPolicy { extract: package.is(booleans::EXTRACT_NATIVE_LIBS), debuggable: package.is(booleans::DEBUGGABLE), manifest_compat_disabled: package.page_size_app_compat_flags == 64, ..policy };
            let library_guest = format!("{guest}/lib");
            let library_host = host.join("lib");
            let libraries = apks.copy_native_libraries_with_override(&package, abis, None, policy, &NativeLibraryDestination {
                guest_root: &library_guest, root: &library_host, owner: GuestInode { uid: Some(1000), gid: Some(1000), mode: None }, zip_time, restorecon,
            }).map_err(|e| e.to_string())?;
            File::open(&host).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
            let metadata = fs::symlink_metadata(&host).map_err(|e| e.to_string())?;
            let receipt = Receipt { guest_path: guest, host_path: host.clone(), device: metadata.dev(), inode: metadata.ino() };
            // Original records this before initPackageTracedLI. A later scan
            // rejection must explicitly remove the code, not invent a receipt.
            self.pending.lock().unwrap().push(receipt.clone());
            Ok(Expanded { package, libraries, receipt })
        })();
        match result { Ok(value) => Ok(Some(value)), Err(error) => {
            let cleanup = fs::remove_dir_all(&parent);
            match cleanup { Ok(()) => Err(error), Err(cleanup) => Err(format!("{error}; compressed code cleanup: {cleanup}")) }
        } }
    }
    /// initPackageTracedLI failed after successful decompression. Never delete
    /// an unrelated replacement that happens to reuse the same path.
    pub fn reject(&self, receipt: &Receipt) -> Result<(), String> {
        self.check(receipt)?;
        fs::remove_dir_all(&receipt.host_path).map_err(|e| e.to_string())?;
        let parent = receipt.host_path.parent().ok_or("Compressed destination parent missing")?;
        fs::remove_dir(parent).map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Called once before lifecycle.system_ready. Successful items are removed
    /// incrementally; a failure retains the current and remaining receipts.
    pub fn release_on_system_ready(&self, release: &dyn Fn(&str, &Path) -> Result<(), String>) -> Result<(), String> {
        let _gate = self.release_gate.lock().unwrap();
        loop {
            let Some(receipt) = self.pending.lock().unwrap().first().cloned() else { break };
            // Failed installs have already removed their code, as in the source.
            match fs::symlink_metadata(&receipt.host_path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {},
                Err(e) => return Err(e.to_string()),
                Ok(_) => { self.check(&receipt)?; release(&receipt.guest_path, &receipt.host_path)?; }
            }
            self.pending.lock().unwrap().remove(0);
        }
        Ok(())
    }
    pub fn release_paths(&self) -> Result<Vec<String>, String> {
        let pending = self.pending.lock().unwrap();
        for receipt in pending.iter() {
            match fs::symlink_metadata(&receipt.host_path) {
                Ok(_) => self.check(receipt)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {},
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(pending.iter().map(|receipt|receipt.guest_path.clone()).collect())
    }
    /// A successful prepareReady Binder reply acknowledges the exact batch sent
    /// to original F2fsUtils; future receipts remain pending.
    pub fn acknowledge_released(&self, paths: &[String]) -> Result<(), String> {
        let _gate = self.release_gate.lock().unwrap();
        let mut pending = self.pending.lock().unwrap();
        if pending.len() < paths.len() || pending.iter().zip(paths).any(|(receipt,path)|receipt.guest_path!=*path) {
            return Err("Compressed release acknowledgment differs from retained receipts".into());
        }
        pending.drain(..paths.len());
        Ok(())
    }
    pub fn pending_receipts(&self) -> Vec<Receipt> { self.pending.lock().unwrap().clone() }
    fn check(&self, receipt: &Receipt) -> Result<(), String> {
        let metadata = fs::symlink_metadata(&receipt.host_path).map_err(|e| e.to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.dev() != receipt.device || metadata.ino() != receipt.inode { return Err("Compressed destination ownership changed".into()) }
        Ok(())
    }
    fn allocate(&self, package: &str) -> Result<(String, PathBuf, PathBuf), String> {
        if package.is_empty() || package.contains('/') || package.contains('\0') || package == "." || package == ".." { return Err("Invalid compressed package name".into()) }
        loop {
            let outer = format!("~~{}", random_name()?);
            let parent = self.data_app.join(&outer);
            match fs::create_dir(&parent) { Ok(()) => {}, Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue, Err(e) => return Err(e.to_string()) }
            let allocated: Result<(String, PathBuf, PathBuf), String> = (|| { directory_metadata(&parent)?; let inner = format!("{package}-{}", random_name()?); Ok((format!("/data/app/{outer}/{inner}"), parent.join(inner), parent.clone())) })();
            if allocated.is_err() { fs::remove_dir(&parent).map_err(|e| format!("Allocation cleanup: {e}"))?; }
            return allocated;
        }
    }
}
fn directory_metadata(path: &Path) -> Result<(), String> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    record(path, GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o755) }).map_err(|e| e.to_string())
}
fn random_name() -> Result<String, String> {
    let mut bytes = [0u8; 16]; File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes)).map_err(|e| e.to_string())?;
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        result.push(DIGITS[((bits >> 18) & 63) as usize] as char); result.push(DIGITS[((bits >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 { DIGITS[((bits >> 6) & 63) as usize] as char } else { '=' });
        result.push(if chunk.len() > 2 { DIGITS[(bits & 63) as usize] as char } else { '=' });
    }
    Ok(result)
}

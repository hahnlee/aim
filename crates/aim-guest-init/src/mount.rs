//! `/data`, `/metadata` and `/cache`: the data directory's image
//! (docs/storage.md), attached on another thread from the boot's start and
//! mounted for the guest when init's first `mount_all` runs, in the `fs`
//! stage, as a device mounts its partitions there. What runs before only
//! needs the image and the runtime directory.

use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aim_android_image::identity::read_tree_identity;
use aim_android_image::system_server::APP_HOST_SERVICES;
use aim_storage::data::{self, DataImage};
use aim_storage::fscrypt::CeProtector;

use crate::paths::{Layout, Sweep};

pub struct DataMount {
    layout: Layout,
    /// The boot's start.
    epoch: Instant,
    /// Run mode: the attach, until it is waited for, and when it ended.
    pending: Option<JoinHandle<Result<(DataImage, Instant), String>>>,
    /// When the attach ended, and when the mount did and how long it
    /// waited for the attach, since the boot's start.
    attached: Option<Duration>,
    mounted: Option<(Duration, Duration)>,
    /// Declared before the image: finishes before it is detached.
    sweep: Option<Sweep>,
    image: Option<DataImage>,
}

impl DataMount {
    /// Starts attaching the image of `dir` (run mode), or lays the data
    /// directory out in place (`None`, a dry run). A data directory without
    /// an image starts from a template in `userdata` (`data::template`),
    /// for the booted system image and the SKU `named` or, without one, the
    /// Mac's.
    pub fn start(
        layout: Layout,
        dir: Option<PathBuf>,
        userdata: Option<PathBuf>,
        named: Option<String>,
        epoch: Instant,
    ) -> Result<Self, String> {
        let image = layout.image.clone();
        let pending = dir
            .map(|dir| {
                std::thread::Builder::new()
                    .name("data-attach".into())
                    .spawn(move || {
                        let template = userdata
                            .filter(|_| !data::image_of(&dir).exists())
                            .and_then(|userdata| {
                                let identity = read_tree_identity(&image).ok().flatten();
                                let sku = named.or_else(|| crate::sku::host().map(str::to_string));
                                data::template(&userdata, identity.as_deref(), sku.as_deref())
                            });
                        DataImage::attach(&dir, template.as_deref())
                            .map(|image| (image, Instant::now()))
                    })
                    .map_err(|e| format!("data image: {e}"))
            })
            .transpose()?;
        Ok(Self {
            layout,
            epoch,
            pending,
            attached: None,
            mounted: None,
            sweep: None,
            image: None,
        })
    }

    /// Waits for the attach and lays out the persistent directories, once;
    /// returns how long it waited.
    pub fn mount(&mut self) -> Result<Duration, String> {
        let start = Instant::now();
        if self.mounted.is_some() {
            return Ok(Duration::ZERO);
        }
        if let Some(pending) = self.pending.take() {
            let (image, attached) = pending
                .join()
                .map_err(|_| "data image: the attach panicked".to_string())??;
            self.attached = Some(attached - self.epoch);
            if image.dir() != self.layout.data {
                return Err(format!(
                    "data image mounted at {}, not {}",
                    image.dir().display(),
                    self.layout.data.display()
                ));
            }
            self.image = Some(image);
        }
        self.sweep = Some(self.layout.prepare_data().map_err(|e| e.to_string())?);
        let waited = start.elapsed();
        self.mounted = Some((self.epoch.elapsed(), waited));
        Ok(waited)
    }

    /// The data filesystem's fscrypt owner, which the mount prepared.
    pub fn fscrypt(&self) -> Result<aim_storage::fscrypt::Owner, String> {
        aim_storage::fscrypt::Owner::locate(&self.layout.runtime)
            .and_then(|owner| owner.ok_or_else(|| std::io::ErrorKind::NotFound.into()))
            .map_err(|e| format!("fscrypt: {e}"))
    }

    /// Whether `/data` was written without file-based encryption: it has
    /// entries, other than the mount point of `/data/user/0` this mount
    /// makes and the `/data/unencrypted` of an `installkey` that never got
    /// further, yet no `mount_all` ever marked it encrypted
    /// (`Owner::encrypt`). Its directories cannot get a policy
    /// (ENOTEMPTY), so the data is recreated, never mixed (ADR 0016, the
    /// breaking migration).
    pub fn predates_fbe(&self) -> Result<bool, String> {
        let data = self.layout.data.join("data");
        let failed = |e: std::io::Error| format!("{}: {e}", data.display());
        let written = std::fs::read_dir(&data)
            .map_err(failed)?
            .map(|entry| entry.map(|e| e.file_name()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(failed)?
            .iter()
            .any(|name| name != "user" && name != "unencrypted");
        Ok(written && !self.fscrypt()?.has_policies().map_err(failed)?)
    }

    /// Claims the protection of the users' CE keys for the image's
    /// LockSettings (ADR 0016): the app host's in an app host image
    /// (`APP_HOST_SERVICES`), else the original LockSettingsService. Each
    /// keeps the protector of a CE key where the other cannot open it (a
    /// keychain item; a synthetic password blob in `system_de`), so a data
    /// directory the other protected is refused here rather than leaving
    /// its users locked (Direct Boot).
    pub fn claim_ce_protection(&self) -> Result<(), String> {
        let image = aim_android_init::ImageRoot::new(&self.layout.image);
        let protector = match image.host_path(APP_HOST_SERVICES).try_exists() {
            Ok(true) => CeProtector::AppHost,
            Ok(false) => CeProtector::Original,
            Err(e) => return Err(format!("{APP_HOST_SERVICES}: {e}")),
        };
        match self.fscrypt()?.claim_ce_protection(protector) {
            Ok(None) => Ok(()),
            Ok(Some(other)) => Err(format!(
                "{}: its CE storage is protected by {other}, and this image's is {protector}, which cannot unlock it: boot it with an image of {other}, or use another data directory (ADR 0016)",
                self.layout.data.display()
            )),
            Err(e) => Err(format!("fscrypt: {e}")),
        }
    }

    /// Places the data template's files (`data::TEMPLATE_FILES`) in the
    /// volume once their directories exist with their policies, after
    /// `init_user0`: each moved inode takes its new directory's policy, as
    /// the files the template's owners wrote at its first boot did. A boot
    /// that stops part way places the rest at the next. Returns how many
    /// files and directories it moved.
    pub fn place_template(&self) -> Result<usize, String> {
        let staged = self.layout.data.join(data::TEMPLATE_FILES);
        if !staged.exists() {
            return Ok(0);
        }
        let failed = |e: std::io::Error| format!("data template: {e}");
        let moved = place(&staged, &self.layout.data).map_err(failed)?;
        std::fs::remove_dir_all(&staged).map_err(failed)?;
        Ok(moved)
    }

    /// The attach's and the mount's timeline entries.
    pub fn timeline(&self) -> Vec<(Duration, String)> {
        let mut out = Vec::new();
        if let Some(at) = self.attached {
            out.push((at, "data image attached".to_string()));
        }
        if let Some((at, waited)) = self.mounted {
            out.push((
                at,
                format!("data mounted, after waiting {:.3} s", waited.as_secs_f64()),
            ));
        }
        out
    }

    /// Leaves the attachment to the process owner after this boot has
    /// stopped all guest and native owners. Detaching inside guest-init can
    /// still be rejected by process-generation mappings which only retire at
    /// exec or exit. The next exec generation, or aimctl after exit, performs
    /// the normal exact-image detach.
    pub fn handoff_detach(&mut self) -> Result<(), String> {
        if self.pending.is_some() {
            return Err("DATA detach handoff before mount".into());
        }
        if let Some(image) = self.image.as_mut() {
            image.handoff_detach()?;
        }
        Ok(())
    }
}

/// Moves what `from` holds into `to`, merging into the directories `to`
/// already has. Each entry takes its policies before it moves, so one that
/// is in `to` has them; a boot that stops part way adopts what is left
/// again at the next.
fn place(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<usize> {
    let mut moved = 0;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir()
            && std::fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir())
        {
            moved += place(&source, &target)?;
        } else {
            moved += adopt(to, &source)?;
            std::fs::rename(&source, &target)?;
        }
    }
    Ok(moved)
}

/// `entry`, to be moved into `dir`, and what it holds take the policies
/// they would have had if made there.
fn adopt(dir: &std::path::Path, entry: &std::path::Path) -> std::io::Result<usize> {
    aim_storage::fscrypt::inherit(dir, entry)?;
    let mut adopted = 1;
    if std::fs::symlink_metadata(entry)?.is_dir() {
        for child in std::fs::read_dir(entry)? {
            adopted += adopt(entry, &child?.path())?;
        }
    }
    Ok(adopted)
}

impl Drop for DataMount {
    /// An attach still running finishes, and its image is detached.
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            let _ = pending.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A /data written without policies is refused, so it is recreated,
    /// never mixed; a new one, or one that has a policy, is not.
    #[test]
    fn a_data_directory_from_before_fbe_is_refused() {
        let dir = std::env::temp_dir().join(format!("aim-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let layout = Layout::new(dir.join("image"), dir.join("data"), Some(dir.join("run")));
        std::fs::create_dir_all(&layout.runtime).unwrap();
        let mut mount = DataMount::start(layout, None, None, None, Instant::now()).unwrap();
        mount.mount().unwrap();
        assert!(!mount.predates_fbe().unwrap());
        let unencrypted = dir.join("data/data/unencrypted");
        std::fs::create_dir(&unencrypted).unwrap();
        assert!(!mount.predates_fbe().unwrap());
        let system = dir.join("data/data/system");
        std::fs::create_dir_all(system.join("users")).unwrap();
        assert!(mount.predates_fbe().unwrap());
        let misc = dir.join("data/data/misc");
        std::fs::create_dir(&misc).unwrap();
        let policy = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16]].concat();
        mount.fscrypt().unwrap().set_policy(&misc, &policy).unwrap();
        assert!(!mount.predates_fbe().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A boot that ends after `mount_all` accepted a new /data, its first
    /// encrypted directory made but given no policy yet, leaves a /data
    /// the next boot accepts.
    #[test]
    fn a_data_directory_marked_encrypted_is_not_taken_for_one_from_before() {
        let dir = std::env::temp_dir().join(format!("aim-marked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let layout = Layout::new(dir.join("image"), dir.join("data"), Some(dir.join("run")));
        std::fs::create_dir_all(&layout.runtime).unwrap();
        let mut mount = DataMount::start(layout, None, None, None, Instant::now()).unwrap();
        mount.mount().unwrap();
        assert!(!mount.predates_fbe().unwrap());
        mount.fscrypt().unwrap().encrypt().unwrap();
        std::fs::create_dir(dir.join("data/data/bootchart")).unwrap();
        assert!(!mount.predates_fbe().unwrap());
        // Marking again keeps what was recorded.
        let misc = dir.join("data/data/misc");
        std::fs::create_dir(&misc).unwrap();
        let policy = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16]].concat();
        let owner = mount.fscrypt().unwrap();
        owner.set_policy(&misc, &policy).unwrap();
        owner.encrypt().unwrap();
        assert!(owner.locking());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The first image to mount a data directory protects its CE storage;
    /// an image whose LockSettings is the other's is refused.
    #[test]
    fn a_data_directory_stays_with_its_ce_protection() {
        let dir = std::env::temp_dir().join(format!("aim-ce-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mount = |image: &str| {
            let layout = Layout::new(dir.join(image), dir.join("data"), Some(dir.join("run")));
            std::fs::create_dir_all(&layout.runtime).unwrap();
            let mut mount = DataMount::start(layout, None, None, None, Instant::now()).unwrap();
            mount.mount().unwrap();
            mount
        };
        let services = dir.join("app-host").join(&APP_HOST_SERVICES[1..]);
        std::fs::create_dir_all(services.parent().unwrap()).unwrap();
        std::fs::write(services, "vold\n").unwrap();
        std::fs::create_dir_all(dir.join("derived/system")).unwrap();
        mount("derived").claim_ce_protection().unwrap();
        mount("derived").claim_ce_protection().unwrap();
        let refused = mount("app-host").claim_ce_protection().unwrap_err();
        assert!(
            refused.contains("protected by the original LockSettingsService"),
            "{refused}"
        );
        std::fs::remove_dir_all(dir.join("data")).unwrap();
        mount("app-host").claim_ce_protection().unwrap();
        assert!(mount("derived").claim_ce_protection().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A template's files merge into the directories init and vold made,
    /// each taking its new directory's policy; what is new to a directory
    /// moves whole, and everything under it is adopted too.
    #[test]
    fn template_files_take_the_policies_of_where_they_land() {
        let dir = std::env::temp_dir().join(format!("aim-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (staged, volume) = (dir.join("template"), dir.join("volume"));
        for path in ["data/system/users/0", "data/app/~~a/pkg-b/oat"] {
            std::fs::create_dir_all(staged.join(path)).unwrap();
        }
        std::fs::write(staged.join("data/system/packages.xml"), "packages").unwrap();
        std::fs::write(staged.join("data/app/~~a/pkg-b/base.apk"), "apk").unwrap();
        for path in ["data/system", "data/app"] {
            std::fs::create_dir_all(volume.join(path)).unwrap();
        }
        let policy = |path: &str| aim_storage::fscrypt::context(&volume.join(path)).unwrap();
        // An encrypted /data/system: a v2 context, as the layer stores it.
        let context = [&[2u8, 1, 4, 2, 0, 0, 0, 0][..], &[7; 16], &[0; 16]].concat();
        let system =
            std::ffi::CString::new(volume.join("data/system").as_os_str().as_bytes()).unwrap();
        let name = aim_storage::fscrypt::CONTEXT;
        assert_eq!(
            unsafe {
                libc::setxattr(
                    system.as_ptr(),
                    name.as_ptr(),
                    context.as_ptr().cast(),
                    context.len(),
                    0,
                    0,
                )
            },
            0
        );
        assert_eq!(place(&staged, &volume).unwrap(), 7);
        for path in [
            "data/system/packages.xml",
            "data/system/users",
            "data/system/users/0",
        ] {
            let placed = policy(path).unwrap();
            assert_eq!(placed[..24], context[..24], "{path}");
            assert_ne!(placed[24..], context[24..], "{path}: its own nonce");
        }
        // /data/app has no policy here, so neither has what moved into it.
        assert_eq!(policy("data/app/~~a/pkg-b/base.apk"), None);
        assert!(volume.join("data/app/~~a/pkg-b/oat").is_dir());
        // An entry that cannot move stays staged, with its policy already:
        // nothing in the volume lacks one.
        std::fs::write(staged.join("data/system/settings.xml"), "settings").unwrap();
        std::fs::create_dir_all(volume.join("data/system/settings.xml/x")).unwrap();
        assert!(place(&staged, &volume).is_err());
        let left = aim_storage::fscrypt::context(&staged.join("data/system/settings.xml"))
            .unwrap()
            .unwrap();
        assert_eq!(left[..24], context[..24]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

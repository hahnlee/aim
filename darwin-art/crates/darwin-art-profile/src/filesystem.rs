use crate::{ProfileError, ProfilePaths};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Command;

const MARKER: &str = ".darwin-art-profile-volume-v1";

pub(crate) struct ProfileFilesystem {
    paths: ProfilePaths,
}

impl ProfileFilesystem {
    pub(crate) fn new(paths: ProfilePaths) -> Self {
        Self { paths }
    }

    pub(crate) fn ensure(&self) -> Result<&Path, ProfileError> {
        secure_directory(&self.paths.profiles_root)?;
        secure_directory(&self.paths.profile_root)?;
        reject_symlink(&self.paths.image)?;
        reject_symlink(&self.paths.mount)?;
        secure_directory(&self.paths.mount)?;
        if !self.is_mounted()? {
            if !self.paths.image.exists() {
                self.create_image()?;
            }
            self.attach()?;
        }
        self.initialize_volume()?;
        self.verify_case_sensitive()?;
        Ok(&self.paths.mount)
    }

    pub(crate) fn is_mounted(&self) -> Result<bool, ProfileError> {
        if !self.paths.mount.exists() {
            return Ok(false);
        }
        let mount = fs::metadata(&self.paths.mount)?;
        let parent = fs::metadata(&self.paths.profile_root)?;
        Ok(mount.dev() != parent.dev())
    }

    pub(crate) fn detach(&self) -> Result<(), ProfileError> {
        if !self.is_mounted()? {
            if self.paths.image.exists() {
                self.compact_image()?;
            }
            return Ok(());
        }
        run_hdiutil(["detach".as_ref(), self.paths.mount.as_os_str()])?;
        self.compact_image()?;
        Ok(())
    }

    fn compact_image(&self) -> Result<(), ProfileError> {
        // Compaction is safe only while detached. It releases APFS sparsebundle
        // bands that no longer contain Android profile data.
        run_hdiutil(["compact".as_ref(), self.paths.image.as_os_str()])?;
        Ok(())
    }

    fn create_image(&self) -> Result<(), ProfileError> {
        let staging = self.paths.profile_root.join(format!(
            "android-data.creating-{}.sparsebundle",
            std::process::id()
        ));
        if staging.exists() {
            return Err(ProfileError::Daemon(format!(
                "staging image already exists: {}",
                staging.display()
            )));
        }
        let size = env::var_os("DARWIN_ART_PROFILE_IMAGE_SIZE").unwrap_or_else(|| "64g".into());
        let volume = format!("DarwinART-{}", self.paths.profile_id);
        let result = run_hdiutil([
            "create".as_ref(),
            "-type".as_ref(),
            "SPARSEBUNDLE".as_ref(),
            "-size".as_ref(),
            size.as_os_str(),
            "-layout".as_ref(),
            "NONE".as_ref(),
            staging.as_os_str(),
        ]);
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        // hdiutil cannot directly format a sparse bundle as APFSX on current
        // macOS releases. Attach the just-created blank image and format only
        // that validated device with diskutil's case-sensitive personality.
        let attach = run_command(
            "/usr/bin/hdiutil",
            ["attach".as_ref(), "-nomount".as_ref(), staging.as_os_str()],
        )?;
        let device = String::from_utf8_lossy(&attach)
            .split_whitespace()
            .find(|field| is_whole_disk(field))
            .ok_or_else(|| ProfileError::Daemon("hdiutil returned no whole disk".into()))?
            .to_owned();
        let format = run_command(
            "/usr/sbin/diskutil",
            [
                "eraseDisk".as_ref(),
                "APFSX".as_ref(),
                volume.as_ref(),
                "GPT".as_ref(),
                device.as_ref(),
            ],
        );
        let detach = run_hdiutil(["detach".as_ref(), device.as_ref()]);
        if let Err(error) = format {
            let _ = detach;
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        detach?;
        // `diskutil eraseDisk` initializes APFS and can leave every
        // sparsebundle band allocated although the new profile is empty.
        // Compact before publication so physical usage follows Android
        // /data contents instead of the logical image capacity.
        run_hdiutil(["compact".as_ref(), staging.as_os_str()])?;
        fs::rename(&staging, &self.paths.image)?;
        Ok(())
    }

    fn attach(&self) -> Result<(), ProfileError> {
        run_hdiutil([
            "attach".as_ref(),
            self.paths.image.as_os_str(),
            "-mountpoint".as_ref(),
            self.paths.mount.as_os_str(),
            "-nobrowse".as_ref(),
            "-noautoopen".as_ref(),
            "-owners".as_ref(),
            "off".as_ref(),
        ])?;
        Ok(())
    }

    fn initialize_volume(&self) -> Result<(), ProfileError> {
        let marker = self.paths.mount.join(MARKER);
        if !marker.exists() {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)?
                .write_all(b"darwin-art profile volume v1\n")?;
        }
        for relative in [
            "data/apps",
            "data/user/0",
            "packages",
            "storage/emulated/0",
            "run",
            "system",
        ] {
            secure_directory(&self.paths.mount.join(relative))?;
        }
        Ok(())
    }

    fn verify_case_sensitive(&self) -> Result<(), ProfileError> {
        let probe = self.paths.mount.join("run/case-probe");
        fs::create_dir(&probe).or_else(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                fs::remove_dir_all(&probe)?;
                fs::create_dir(&probe)
            } else {
                Err(error)
            }
        })?;
        let upper = probe.join("CaseProbe");
        let lower = probe.join("caseprobe");
        let result = (|| -> io::Result<()> {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&upper)?;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lower)?;
            Ok(())
        })();
        let _ = fs::remove_dir_all(&probe);
        result.map_err(|error| {
            ProfileError::Daemon(format!(
                "profile volume is not case-sensitive (APFSX required): {error}"
            ))
        })
    }
}

fn secure_directory(path: &Path) -> Result<(), ProfileError> {
    reject_symlink(path)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ProfileError::Daemon(format!(
            "refusing symlink in profile control path: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn run_hdiutil<const N: usize>(arguments: [&std::ffi::OsStr; N]) -> Result<(), ProfileError> {
    run_command("/usr/bin/hdiutil", arguments).map(|_| ())
}

fn run_command<const N: usize>(
    program: &str,
    arguments: [&std::ffi::OsStr; N],
) -> Result<Vec<u8>, ProfileError> {
    let output = crate::output_owned(Command::new(program).args(arguments))?;
    if !output.status.success() {
        return Err(ProfileError::Daemon(format!(
            "{} failed: {}",
            Path::new(program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn is_whole_disk(value: &str) -> bool {
    value.strip_prefix("/dev/disk").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

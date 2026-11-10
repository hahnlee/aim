//! init's file-based encryption (android-16.0.0_r1 `init/builtins.cpp`,
//! `init/fscrypt_init_extensions.cpp`, fs_mgr's `fileencryption=` and
//! libfscrypt's options): whether `/data` is file-encrypted, `mkdir`'s
//! `encryption=` and `key=` options, and the policy init gives a directory
//! from the references vold's `fbeEnable` wrote under `/data/unencrypted`.
//! The policies are set through the data filesystem's fscrypt owner
//! (`aim_storage::fscrypt`, ADR 0016), as init sets them by ioctl.

use std::fs;
use std::path::Path;

use aim_storage::fscrypt::{DIRECT_KEY, IV_INO_LBLK_32, IV_INO_LBLK_64, Owner};

/// `fscrypt_key_ref` and `fscrypt_key_per_boot_ref`, under `/data`.
pub const KEY_REF: &str = "/unencrypted/ref";
pub const PER_BOOT_REF: &str = "/unencrypted/per_boot_ref";
/// `fscrypt_key_mode`, under `/data`.
pub const KEY_MODE: &str = "/unencrypted/mode";

const MODE_AES_256_XTS: u8 = 1;
const MODE_AES_256_CTS: u8 = 4;
const MODE_ADIANTUM: u8 = 9;
const MODE_AES_256_HCTR2: u8 = 10;
const MODE_AES_256_HEH: u8 = 126;
const MODE_PRIVATE: u8 = 127;
const PAD_4: u8 = 0x00;
const PAD_16: u8 = 0x02;
const API_Q: u32 = 29;

/// The `fileencryption=` options of the fstab's `/data` entry, none when
/// it has no such flag (fs_mgr's `file_encryption`).
pub fn data_file_encryption(fstab: &str) -> Option<&str> {
    fstab
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .find(|fields| {
            fields.first().is_some_and(|f| !f.starts_with('#')) && fields.get(1) == Some(&"/data")
        })?
        .get(4)?
        .split(',')
        .find_map(|flag| flag.strip_prefix("fileencryption="))
}

/// `FscryptAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Attempt,
    Require,
    DeleteIfNecessary,
}

/// `ParseMkdir`'s options after the group: the action and the reference
/// (`KEY_REF` or `PER_BOOT_REF`).
pub fn mkdir_options(options: &[String]) -> Result<(Action, &'static str), String> {
    let (mut action, mut reference) = (None, None);
    for option in options {
        if let Some(value) = option.strip_prefix("encryption=") {
            if action.is_some() {
                return Err(format!("Duplicated option: '{option}'"));
            }
            action = Some(match value {
                "None" => Action::None,
                "Attempt" => Action::Attempt,
                "Require" => Action::Require,
                "DeleteIfNecessary" => Action::DeleteIfNecessary,
                _ => return Err(format!("Unknown encryption option: '{option}'")),
            });
        } else if let Some(value) = option.strip_prefix("key=") {
            if reference.is_some() {
                return Err(format!("Duplicated option: '{option}'"));
            }
            reference = Some(match value {
                "ref" => KEY_REF,
                "per_boot_ref" => PER_BOOT_REF,
                _ => return Err(format!("Unknown key option: '{option}'")),
            });
        } else {
            return Err(format!("Unknown option: '{option}'"));
        }
    }
    let action = action.unwrap_or(Action::None);
    if reference.is_some() && action == Action::None {
        return Err("Key option set but encryption action is none".to_string());
    }
    Ok((action, reference.unwrap_or(KEY_REF)))
}

/// libfscrypt's `ParseOptionsForApiLevel` and `EnsurePolicy`'s kernel
/// policy: a `struct fscrypt_policy_v1` or `_v2` of the options `text`
/// naming the key `key_ref`.
pub fn policy(first_api_level: u32, text: &str, key_ref: &[u8]) -> Result<Vec<u8>, String> {
    let bad = |what: &str| format!("{what}: invalid encryption options '{text}'");
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return Err(bad("too many fields"));
    }
    let mut version = if first_api_level > API_Q { 2 } else { 1 };
    let mut contents = MODE_AES_256_XTS;
    let mut filenames = MODE_AES_256_CTS;
    let (mut flags, mut dusize_4k) = (0, false);
    if !parts[0].is_empty() {
        contents = match parts[0] {
            "aes-256-xts" | "software" => MODE_AES_256_XTS,
            "adiantum" => MODE_ADIANTUM,
            "ice" => MODE_PRIVATE,
            _ => return Err(bad("contents mode")),
        };
    }
    if contents == MODE_ADIANTUM {
        filenames = MODE_ADIANTUM;
    }
    if let Some(name) = parts.get(1).filter(|p| !p.is_empty()) {
        filenames = match *name {
            "aes-256-cts" => MODE_AES_256_CTS,
            "aes-256-heh" => MODE_AES_256_HEH,
            "adiantum" => MODE_ADIANTUM,
            "aes-256-hctr2" => MODE_AES_256_HCTR2,
            _ => return Err(bad("file names mode")),
        };
    }
    for flag in parts.get(2).iter().flat_map(|p| p.split('+')) {
        match flag {
            "" | "wrappedkey_v0" => {}
            "v1" => version = 1,
            "v2" => version = 2,
            "inlinecrypt_optimized" => flags |= IV_INO_LBLK_64,
            "emmc_optimized" => flags |= IV_INO_LBLK_32,
            "dusize_4k" => dusize_4k = true,
            _ => return Err(bad("unknown flag")),
        }
    }
    // Pre-Q devices kept 4-byte padding for v1 AES-256-CTS names.
    flags |= if first_api_level <= API_Q && version == 1 && filenames == MODE_AES_256_CTS {
        PAD_4
    } else {
        PAD_16
    };
    match (contents == MODE_ADIANTUM, filenames == MODE_ADIANTUM) {
        (true, true) => flags |= DIRECT_KEY,
        (false, false) => {}
        _ => return Err(bad("Adiantum must be both modes or neither")),
    }
    if [IV_INO_LBLK_64, IV_INO_LBLK_32, DIRECT_KEY]
        .iter()
        .filter(|flag| flags & **flag != 0)
        .count()
        > 1
    {
        return Err(bad("more than one IV generation method"));
    }
    match (version, key_ref.len()) {
        (1, 8) => Ok([&[0, contents, filenames, flags][..], key_ref].concat()),
        (2, 16) => {
            let log2_data_unit = if dusize_4k { 12 } else { 0 };
            Ok([
                &[2, contents, filenames, flags, log2_data_unit, 0, 0, 0][..],
                key_ref,
            ]
            .concat())
        }
        _ => Err(format!(
            "a key reference of {} bytes for a v{version} policy",
            key_ref.len()
        )),
    }
}

/// `FscryptSetDirectoryPolicy`: `dir` given the policy of the key whose
/// reference is the file `reference` with the options in `mode` (both
/// host paths under `/data/unencrypted`), as `action` asks.
pub fn set_directory_policy(
    owner: &Owner,
    first_api_level: u32,
    reference: &Path,
    mode: &Path,
    action: Action,
    dir: &Path,
) -> Result<(), String> {
    if action == Action::None {
        return Ok(());
    }
    let key_ref = fs::read(reference).map_err(|e| {
        format!(
            "Unable to read system policy with name {}: {e}",
            reference.display()
        )
    })?;
    let options = fs::read_to_string(mode)
        .map_err(|e| format!("Cannot read encryption options string: {e}"))?;
    let policy = policy(first_api_level, &options, &key_ref)?;
    let ensure = || owner.set_policy(dir, &policy);
    let Err(error) = ensure() else {
        return Ok(());
    };
    let failed = format!("Setting policy on {} failed: {error}", dir.display());
    match action {
        Action::Attempt => Ok(()),
        Action::DeleteIfNecessary => {
            for entry in fs::read_dir(dir).map_err(|e| format!("{failed}; {e}"))? {
                let path = entry.map_err(|e| format!("{failed}; {e}"))?.path();
                let removed = if path.is_dir() && !path.is_symlink() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                removed.map_err(|e| format!("{failed}; {}: {e}", path.display()))?;
            }
            ensure().map_err(|e| format!("{failed}; again after deleting its contents: {e}"))
        }
        Action::None | Action::Require => Err(failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_entry_names_its_file_encryption_options() {
        let fstab = "# comment /data\n\
            /dev/block/userdata  /data  ext4  noatime,nosuid  wait,latemount,fileencryption=aes-256-xts:aes-256-cts:v2\n";
        assert_eq!(
            data_file_encryption(fstab),
            Some("aes-256-xts:aes-256-cts:v2")
        );
        assert_eq!(
            data_file_encryption("/dev/block/userdata /data ext4 noatime wait,latemount\n"),
            None
        );
        assert_eq!(
            data_file_encryption("/dev/x /metadata ext4 noatime fileencryption=a\n"),
            None
        );
    }

    #[test]
    fn mkdir_options_are_parsed_as_parse_mkdir_does() {
        let options =
            |o: &[&str]| mkdir_options(&o.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(options(&[]), Ok((Action::None, KEY_REF)));
        assert_eq!(
            options(&["encryption=Require", "key=per_boot_ref"]),
            Ok((Action::Require, PER_BOOT_REF))
        );
        assert_eq!(
            options(&["encryption=DeleteIfNecessary"]),
            Ok((Action::DeleteIfNecessary, KEY_REF))
        );
        assert!(
            options(&["key=ref"])
                .unwrap_err()
                .contains("encryption action is none")
        );
        assert!(
            options(&["encryption=Maybe"])
                .unwrap_err()
                .contains("Unknown encryption")
        );
        assert!(options(&["key=other", "encryption=Require"]).is_err());
        assert!(
            options(&["encryption=None", "encryption=None"])
                .unwrap_err()
                .contains("Duplicated")
        );
        assert!(options(&["mode=1"]).unwrap_err().contains("Unknown option"));
    }

    #[test]
    fn options_make_the_kernel_policy_libfscrypt_makes() {
        let id = [7u8; 16];
        assert_eq!(
            policy(36, "aes-256-xts:aes-256-cts:v2", &id).unwrap(),
            [&[2, 1, 4, PAD_16, 0, 0, 0, 0][..], &id].concat()
        );
        assert_eq!(
            policy(
                36,
                "aes-256-xts:aes-256-cts:v2+inlinecrypt_optimized+dusize_4k",
                &id
            )
            .unwrap(),
            [&[2, 1, 4, PAD_16 | IV_INO_LBLK_64, 12, 0, 0, 0][..], &id].concat()
        );
        assert_eq!(
            policy(36, "adiantum::v2", &id).unwrap()[..4],
            [2, 9, 9, PAD_16 | DIRECT_KEY]
        );
        // A Q device's v1 policy keeps 4-byte name padding.
        assert_eq!(
            policy(29, "aes-256-xts", &[1; 8]).unwrap(),
            [&[0, 1, 4, PAD_4][..], &[1; 8]].concat()
        );
        assert!(policy(36, "aes-256-xts:aes-256-cts:v2", &[1; 8]).is_err());
        assert!(policy(36, "adiantum:aes-256-cts:v2", &id).is_err());
        assert!(policy(36, "aes-256-xts:aes-256-cts:v2+fast", &id).is_err());
    }

    /// A directory gets the policy of the referenced key once, keeps it,
    /// and a non-empty one is refused unless the action deletes its
    /// contents.
    #[test]
    fn a_directory_gets_the_referenced_policy_as_the_action_asks() {
        let dir = std::env::temp_dir().join(format!("aim-init-fscrypt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (runtime, data) = (dir.join("run"), dir.join("data"));
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(data.join("unencrypted")).unwrap();
        Owner::prepare(&runtime, &data).unwrap();
        let owner = Owner::locate(&runtime).unwrap().unwrap();
        let (reference, mode) = (data.join("unencrypted/ref"), data.join("unencrypted/mode"));
        fs::write(&reference, [7u8; 16]).unwrap();
        fs::write(&mode, "aes-256-xts:aes-256-cts:v2").unwrap();
        let set = |action, target: &Path| {
            set_directory_policy(&owner, 36, &reference, &mode, action, target)
        };
        let misc = data.join("misc");
        fs::create_dir(&misc).unwrap();
        set(Action::Require, &misc).unwrap();
        let context = aim_storage::fscrypt::context(&misc).unwrap().unwrap();
        assert_eq!(
            context[..24],
            policy(36, "aes-256-xts:aes-256-cts:v2", &[7; 16]).unwrap()
        );
        assert!(owner.has_policies().unwrap());
        // The same policy again, once the directory has entries: kept.
        fs::create_dir(misc.join("vold")).unwrap();
        aim_storage::fscrypt::inherit(&misc, &misc.join("vold")).unwrap();
        set(Action::Require, &misc).unwrap();
        // Another key's: EEXIST, which Attempt ignores.
        fs::write(&reference, [8u8; 16]).unwrap();
        assert!(set(Action::Require, &misc).unwrap_err().contains("failed"));
        set(Action::Attempt, &misc).unwrap();
        // A legacy directory with entries and no policy is refused...
        let legacy = data.join("system");
        fs::create_dir_all(legacy.join("users")).unwrap();
        assert!(set(Action::Require, &legacy).is_err());
        assert_eq!(aim_storage::fscrypt::context(&legacy).unwrap(), None);
        set(Action::None, &legacy).unwrap();
        // ...unless its contents may be deleted.
        set(Action::DeleteIfNecessary, &legacy).unwrap();
        assert!(fs::read_dir(&legacy).unwrap().next().is_none());
        assert!(aim_storage::fscrypt::context(&legacy).unwrap().is_some());
        fs::remove_dir_all(dir).unwrap();
    }
}

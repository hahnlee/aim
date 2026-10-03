//! PackageManager (`package`, ADR 0013's core). A native PackageManager
//! serves the state the original persists and keeps it in the original's
//! files, so either can run on the same data (docs/first-boot.md: a data
//! directory starts from the original's own first-boot state). So far this
//! module reads that state as the original reads it at boot
//! (`Settings.readLPw`, the permission service's `AccessPersistence`), from
//! the guest's `/data`. The shared libraries are not in these files: the
//! original builds them at its scan (#707). Sources are read at
//! `android-16.0.0_r1`.

pub mod apps_filter;
pub mod bootstrap;
pub mod component_resolver;
pub mod domain_verification;
pub mod intent;
pub mod intent_filter;
pub mod info;
pub mod intent_resolver;
pub mod feed;
pub mod list;
pub mod libraries;
pub mod model;
pub mod owner;
pub mod parse;
pub mod pkg;
pub mod preferred;
pub mod query;
pub mod reply;
pub mod permissions;
pub mod resolve;
pub mod restrictions;
pub mod settings;
pub mod service;
pub mod scan;
pub mod scan_snapshot;
pub mod sign;
pub mod system_config;
pub mod uri;
pub mod write;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use aim_android_xml::Element;

use permissions::{AccessSystem, AccessUser, RuntimePermissions};
use restrictions::{Restrictions, UserState};
use settings::Settings;

/// The permission service's directories (`PermissionApex`): the system's,
/// and each user's below `misc_de/<user>`.
const PERMISSION_DIR: &str = "apexdata/com.android.permission";

/// What the original reads at boot.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub settings: Settings,
    /// `packages.list`, which the original writes from the settings and
    /// never reads.
    pub list: Vec<list::Entry>,
    /// `None` until the permission service first wrote it (it then
    /// migrates the legacy state).
    pub access: Option<AccessSystem>,
    /// Each user's state, in the order asked for.
    pub users: Vec<(u32, User)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct User {
    /// Every package of the settings, with the state the file gives it or
    /// the state the original assumes without one.
    pub restrictions: Restrictions,
    pub access: Option<AccessUser>,
    pub runtime_permissions: Option<RuntimePermissions>,
}

impl State {
    /// The state below `data` (the guest's `/data`) of the users `users`;
    /// `None` without settings, which makes the original's boot a first
    /// boot.
    pub fn read(data: &Path, users: &[u32]) -> Result<Option<State>, String> {
        let system = data.join("system");
        let Some(settings) = resilient(
            &system.join("packages.xml"),
            &system.join("packages-backup.xml"),
            Settings::parse,
        )?
        else {
            return Ok(None);
        };
        let list = journaled(&system.join("packages.list"))?
            .map(|text| list::parse(&text))
            .transpose()?
            .unwrap_or_default();
        let access = atomic(
            &data.join("misc").join(PERMISSION_DIR).join("access.abx"),
            AccessSystem::parse,
        )?;
        let users = users
            .iter()
            .map(|&user| Ok((user, read_user(data, user, &settings)?)))
            .collect::<Result<_, String>>()?;
        Ok(Some(State {
            settings,
            list,
            access,
            users,
        }))
    }
}

fn read_user(data: &Path, user: u32, settings: &Settings) -> Result<User, String> {
    let dir = data.join("system/users").join(user.to_string());
    let file = resilient(
        &dir.join("package-restrictions.xml"),
        &dir.join("package-restrictions-backup.xml"),
        Restrictions::parse,
    )?;
    // A package the settings do not know is dropped, one the file does not
    // know has the default state; a first install time the file lacks is
    // the settings' legacy one.
    let legacy_times = file.is_some();
    let mut restrictions = file.unwrap_or_default();
    let mut states: HashMap<String, UserState> = restrictions.packages.drain(..).collect();
    restrictions.packages = settings
        .packages
        .iter()
        .map(|p| {
            let mut state = states.remove(&p.name).unwrap_or_else(|| {
                if legacy_times {
                    UserState::default()
                } else {
                    UserState::initialized()
                }
            });
            if legacy_times && state.first_install_time == 0 {
                state.first_install_time = p.legacy_first_install_time;
            }
            (p.name.clone(), state)
        })
        .collect();
    let permissions = data
        .join("misc_de")
        .join(user.to_string())
        .join(PERMISSION_DIR);
    Ok(User {
        restrictions,
        access: atomic(&permissions.join("access.abx"), AccessUser::parse)?,
        runtime_permissions: atomic(
            &permissions.join("runtime-permissions.xml"),
            RuntimePermissions::parse,
        )?,
    })
}

/// `path`'s bytes; `None` when it does not exist.
fn bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `path` with `suffix` appended to its name.
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Reads the first of `candidates` that exists and parses, as the
/// original reads each and falls back to the next when one fails; `None`
/// when none exists, an error when none parses (where the original goes
/// on without the state).
fn first<T>(
    candidates: &[PathBuf],
    parse: impl Fn(&Element) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let mut failed = Vec::new();
    for path in candidates {
        let Some(b) = bytes(path)? else { continue };
        match aim_android_xml::read_next(&b).and_then(|root| parse(&root)) {
            Ok(v) => return Ok(Some(v)),
            Err(e) => failed.push(format!("{}: {e}", path.display())),
        }
    }
    if failed.is_empty() {
        Ok(None)
    } else {
        Err(failed.join("; "))
    }
}

/// A `ResilientAtomicFile` (packages.xml, package-restrictions.xml): the
/// backup a write in progress left, which then wins over the others;
/// else the file, else its reserve copy.
fn resilient<T>(
    path: &Path,
    backup: &Path,
    parse: impl Fn(&Element) -> Result<T, String>,
) -> Result<Option<T>, String> {
    if backup.exists() {
        return first(&[backup.to_owned()], parse);
    }
    first(&[path.to_owned(), sibling(path, ".reservecopy")], parse)
}

/// An `AtomicFile` with a reserve copy (`readWithReserveCopy`, the
/// runtime permissions' `readForUser`): the file, or its legacy backup
/// (`.bak`), which replaces it; its reserve copy only when it does not
/// parse.
fn atomic<T>(
    path: &Path,
    parse: impl Fn(&Element) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let backup = sibling(path, ".bak");
    let file = if backup.exists() {
        backup
    } else {
        path.to_owned()
    };
    if !file.exists() {
        return Ok(None);
    }
    first(&[file, sibling(path, ".reservecopy")], parse)
}

/// A `JournaledFile` (packages.list): the file, or the temporary file a
/// write left before replacing it.
fn journaled(path: &Path) -> Result<Option<String>, String> {
    let b = match bytes(path)? {
        Some(b) => Some(b),
        None => bytes(&sibling(path, ".tmp"))?,
    };
    b.map(|b| String::from_utf8(b).map_err(|e| format!("{}: {e}", path.display())))
        .transpose()
}

/// `e`'s child elements named `name`.
fn children<'a>(e: &'a Element, name: &'a str) -> impl Iterator<Item = &'a Element> {
    e.children().filter(move |c| c.name == name)
}

/// `getAttributeValue`, owned.
fn string(e: &Element, name: &str) -> Option<String> {
    e.string(name).map(Cow::into_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_android_xml::{Node, Value, abx};

    /// The original's state as text XML (`FastXmlSerializer`, which writes
    /// every value as a string), written by hand: what the original reads
    /// is the same whether a file is text or binary.
    const PACKAGES: &str = r#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<packages>
  <version sdkVersion="36" databaseVersion="3" buildFingerprint="vendor/product:16/b" fingerprint="f1" />
  <version volumeUuid="primary_physical" sdkVersion="35" databaseVersion="3" fingerprint="f0" />
  <permission-trees><item name="org.example.tree" package="org.example.app" /></permission-trees>
  <permissions>
    <item name="android.permission.INTERNET" package="android" />
    <item name="android.permission.OLD" package="android" protection="3" />
    <item name="org.example.tree.dyn" package="org.example.app" type="dynamic" icon="5" label="Dyn" />
  </permissions>
  <package name="android" codePath="/system/framework/framework-res.apk" publicFlags="1" privateFlags="8" ft="18f0" ut="18f1" it="1234" version="36" targetSdkVersion="36" scannedAsStoppedSystemApp="false" sharedUserId="1000" packageSource="0" loadingProgress="1.0" loadingCompletedTime="0" domainSetId="00000000-0000-0000-0000-000000000001" appMetadataSource="0">
    <sigs count="1" schemeVersion="3"><cert index="0" key="3082aa" /></sigs>
    <proper-signing-keyset identifier="1" />
  </package>
  <package name="org.example.app" codePath="/data/app/~~a/org.example.app-b" nativeLibraryPath="/data/app/~~a/org.example.app-b/lib" primaryCpuAbi="arm64-v8a" publicFlags="-2147483648" privateFlags="0" ft="-1" ut="2a" version="7" targetSdkVersion="35" restrictUpdateHash="a2V5" userId="10213" isSdkLibrary="false" installer="com.android.shell" installerUid="2000" installInitiator="com.android.shell" packageSource="1" isOrphaned="true" loadingProgress="0.5" loadingCompletedTime="10" domainSetId="00000000-0000-0000-0000-000000000002" appMetadataSource="0">
    <uses-static-lib name="org.example.lib" version="12" />
    <uses-sdk-lib name="org.example.sdk" version="3" optional="false" />
    <sigs count="1" schemeVersion="3">
      <cert index="1" key="3082bb" />
      <pastSigs count="2"><cert index="2" key="3082cc" flags="23" /><cert index="1" flags="17" /></pastSigs>
    </sigs>
    <install-initiator-sigs count="1" schemeVersion="3"><cert index="0" /></install-initiator-sigs>
    <proper-signing-keyset identifier="2" />
    <defined-keyset alias="upgrade" identifier="3" />
    <mime-group name="images"><mime-type value="image/png" /><mime-type value="image/gif" /></mime-group>
  </package>
  <package name="org.example.orphan" codePath="/data/app/~~c/org.example.orphan-d" sharedUserId="10500" />
  <package name="org.example.noid" codePath="/data/app/~~e/org.example.noid-f" />
  <updated-package name="org.example.sys" codePath="/system/priv-app/Sys" ft="18f0" ut="18f0" version="1" targetSdkVersion="36" userId="1000" loadingProgress="1.0" loadingCompletedTime="0" appMetadataSource="0" />
  <shared-user name="android.uid.system" userId="1000">
    <sigs count="1" schemeVersion="3"><cert index="0" /></sigs>
  </shared-user>
  <renamed-package new="org.example.new" old="org.example.old" />
  <keyset-settings version="1">
    <keys><public-key identifier="1" value="a2V5" /></keys>
    <keysets><keyset identifier="1"><key-id identifier="1" /></keyset></keysets>
    <lastIssuedKeyId value="1" />
    <lastIssuedKeySetId value="3" />
  </keyset-settings>
  <domain-verifications>
    <active>
      <package-state packageName="android" id="00000000-0000-0000-0000-000000000001" hasAutoVerifyDomains="true" signature="abc">
        <state><domain name="example.com" state="1" /><domain name="other.com" state="2" /><domain name="example.com" state="4" /></state>
        <user-states>
          <user-state userId="0" allowLinkHandling="true"><enabled-hosts><host name="example.com" /><host name="example.com" /><host name="" /></enabled-hosts></user-state>
          <user-state allowLinkHandling="true" />
        </user-states>
        <uri-relative-filter-groups>
          <domain name="example.com"><uri-relative-filter-group action="1"><uri-relative-filter uri-part="1" pattern-type="0" filter="/path" /></uri-relative-filter-group></domain>
        </uri-relative-filter-groups>
      </package-state>
    </active>
    <restored><package-state packageName="org.example.app" id="00000000-0000-0000-0000-000000000002" hasAutoVerifyDomains="false" /></restored>
  </domain-verifications>
  <domain-verifications-legacy>
    <user-states packageName="android"><user-state userId="0" state="2" /><user-state userId="10" state="3" /><user-state userId="0" state="4" /></user-states>
  </domain-verifications-legacy>
</packages>
"#;

    const RESTRICTIONS: &str = r#"<package-restrictions>
  <pkg name="android" ceDataInode="5" />
  <pkg name="org.example.app" ceDataInode="9181" deDataInode="9203" stopped="true" nl="true" enabled="3" enabledCaller="shell:1000" install-reason="4" first-install-time="2b" blockUninstall="true">
    <enabled-components><item name="org.example.app.A" /></enabled-components>
    <disabled-components><item name="org.example.app.B" /><item name="org.example.app.B" /></disabled-components>
  </pkg>
  <pkg name="org.example.unknown" />
  <preferred-activities />
  <default-apps><default-browser packageName="org.example.app" /></default-apps>
</package-restrictions>
"#;

    const ACCESS_SYSTEM: &str = r#"<access>
  <permission-trees><permission name="org.example.tree" packageName="org.example.app" protectionLevel="0" type="0" /></permission-trees>
  <permissions>
    <permission name="android.permission.INTERNET" packageName="android" protectionLevel="0" type="0" />
    <permission name="org.example.tree.dyn" packageName="org.example.app" protectionLevel="0" type="2" icon="5" label="Dyn" />
    <permission name="org.example.config" packageName="android" protectionLevel="0" type="1" />
  </permissions>
</access>
"#;

    const ACCESS_USER: &str = r#"<access>
  <package-versions><package name="org.example.app" version="1" /></package-versions>
  <default-permission-grant fingerprint="vendor/product:16/b" />
  <app-id-permissions>
    <app-id id="10213"><permission name="android.permission.CAMERA" flags="1" /><permission name="android.permission.INTERNET" flags="3" /></app-id>
  </app-id-permissions>
  <app-id-device-permissions>
    <app-id id="10213"><device id="device:1"><permission name="android.permission.CAMERA" flags="0" /></device></app-id>
  </app-id-device-permissions>
  <app-id-app-ops><app-id id="10213"><app-op name="android:camera" mode="1" /></app-id></app-id-app-ops>
  <package-app-ops><package name="org.example.app"><app-op name="android:run_in_background" mode="0" /></package></package-app-ops>
</access>
"#;

    const RUNTIME: &str = "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><runtime-permissions version=\"0\" fingerprint=\"f1?pc_version=1\"><package name=\"org.example.legacy\"><permission name=\"android.permission.CAMERA\" granted=\"true\" flags=\"300\" /><permission name=\"android.permission.READ_CONTACTS\" granted=\"yes\" flags=\"0\" /></package><shared-user name=\"android.uid.system\" /></runtime-permissions>";

    const LIST: &str = "org.example.app 10213 0 /data/user/0/org.example.app default:targetSdkVersion=35 3003 0 7 1 com.android.shell\n";

    /// `e` with its values typed as `TypedXmlSerializer` writes them into
    /// binary XML (`access.abx`'s versions are ints, PackageManager's longs).
    fn typed(e: &Element, access: bool) -> Element {
        let long_version = [
            "package",
            "updated-package",
            "uses-static-lib",
            "uses-sdk-lib",
        ];
        let attrs = e
            .attrs
            .iter()
            .map(|(name, v)| {
                let s = v.string().unwrap().into_owned();
                let value = match (e.name.as_str(), name.as_str()) {
                    (_, "ft" | "ut" | "it" | "loadingCompletedTime" | "first-install-time") => {
                        Value::LongHex(e.long_hex(name).unwrap().unwrap())
                    }
                    (n, "version") if !access && long_version.contains(&n) => {
                        Value::Long(e.long(name).unwrap().unwrap())
                    }
                    (_, "identifier" | "ceDataInode" | "deDataInode")
                    | ("lastIssuedKeyId" | "lastIssuedKeySetId", "value") => {
                        Value::Long(e.long(name).unwrap().unwrap())
                    }
                    ("public-key", "value") | (_, "restrictUpdateHash") => {
                        Value::BytesBase64(e.bytes_base64(name).unwrap().unwrap())
                    }
                    (_, "key") => Value::BytesHex(e.bytes_hex(name).unwrap().unwrap()),
                    (_, "loadingProgress") => Value::Float(e.float(name).unwrap().unwrap()),
                    (_, "protectionLevel") => Value::IntHex(e.int(name).unwrap().unwrap()),
                    _ if s == "true" || s == "false" => Value::Bool(s == "true"),
                    _ => match s.parse() {
                        Ok(i) => Value::Int(i),
                        Err(_) => Value::String(s),
                    },
                };
                (name.clone(), value)
            })
            .collect();
        let content = e
            .children()
            .map(|c| Node::Element(typed(c, access)))
            .collect();
        Element {
            name: e.name.clone(),
            attrs,
            content,
        }
    }

    fn data(name: &str, binary: bool) -> PathBuf {
        let data = std::env::temp_dir().join(format!("aim-package-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&data);
        let write = |path: &str, text: &str, binary: bool| {
            let path = data.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let bytes = if binary {
                let root = aim_android_xml::read(text.as_bytes()).unwrap();
                abx::write(&typed(&root, root.name == "access")).unwrap()
            } else {
                text.as_bytes().to_vec()
            };
            fs::write(path, bytes).unwrap();
        };
        write("system/packages.xml", PACKAGES, binary);
        write(
            "system/users/0/package-restrictions.xml",
            RESTRICTIONS,
            binary,
        );
        write("system/packages.list", LIST, false);
        write(
            "misc/apexdata/com.android.permission/access.abx",
            ACCESS_SYSTEM,
            true,
        );
        write(
            "misc_de/0/apexdata/com.android.permission/access.abx",
            ACCESS_USER,
            true,
        );
        write(
            "misc_de/0/apexdata/com.android.permission/runtime-permissions.xml",
            RUNTIME,
            false,
        );
        data
    }

    #[test]
    fn reads_the_state_the_original_reads() {
        let text = data("text", false);
        let state = State::read(&text, &[0]).unwrap().unwrap();
        let binary = data("binary", true);
        assert_eq!(State::read(&binary, &[0]).unwrap().unwrap(), state);
        for d in [text, binary] {
            fs::remove_dir_all(d).unwrap();
        }

        let s = &state.settings;
        let domains = &s.domain_verification;
        assert_eq!(domains.active.len(), 1);
        assert_eq!(domains.restored.len(), 1);
        assert_eq!(domains.active[0].name, "android");
        assert!(domains.active[0].has_auto_verify_domains);
        assert_eq!(domains.active[0].signature.as_deref(), Some("abc"));
        assert_eq!(
            domains.active[0].domains,
            [
                (Some("example.com".into()), 4),
                (Some("other.com".into()), 2)
            ]
        );
        assert_eq!(domains.active[0].users.len(), 1);
        assert_eq!(domains.active[0].users[0].enabled_hosts, ["example.com"]);
        assert!(domains.active[0].users[0].allow_link_handling);
        let group = &domains.active[0].uri_relative_filter_groups[0].1[0];
        assert_eq!(group.action, 0);
        assert_eq!(group.filters[0].filter, "/path");
        assert_eq!(
            domains.legacy,
            [(Some("android".into()), vec![(0, 4), (10, 3)])]
        );
        assert_eq!(s.versions.len(), 2);
        assert_eq!(
            s.versions[1].volume_uuid.as_deref(),
            Some("primary_physical")
        );
        assert_eq!(s.versions[1].build_fingerprint, None);
        // `fixProtectionLevel`: signatureOrSystem is signature|privileged.
        assert_eq!(s.permissions[1].protection_level, 0x12);
        assert_eq!(s.permissions[2].dynamic, Some((5, Some("Dyn".into()))));
        assert_eq!(s.permission_trees[0].package, "org.example.app");
        // A package of an unknown shared user, and one without an app id,
        // are dropped.
        let names: Vec<_> = s.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["android", "org.example.app"]);
        let (android, app) = (&s.packages[0], &s.packages[1]);
        assert!(android.shared_user && android.app_id == 1000);
        assert_eq!(
            (android.last_modified_time, android.last_update_time),
            (0x18f0, 0x18f1)
        );
        assert_eq!(app.flags, i32::MIN);
        assert_eq!((app.last_modified_time, app.last_update_time), (-1, 0x2a));
        assert_eq!(app.restrict_update_hash.as_deref(), Some(&b"key"[..]));
        assert_eq!(app.loading_progress, 0.5);
        assert_eq!(app.category_hint, -1);
        assert_eq!(app.install_source.installer_uid, 2000);
        assert!(app.install_source.is_orphaned);
        assert_eq!(app.uses_static_libraries, [("org.example.lib".into(), 12)]);
        assert!(!app.uses_sdk_libraries[0].optional);
        // A certificate's key is written once, then named by its index.
        let sigs = app.signatures.as_ref().unwrap();
        assert_eq!(sigs.signatures, [vec![0x30, 0x82, 0xbb]]);
        assert_eq!(
            sigs.past_signatures,
            Some(vec![
                (vec![0x30, 0x82, 0xcc], 23),
                (vec![0x30, 0x82, 0xbb], 17)
            ])
        );
        let initiator = app.install_source.initiating_package_signatures.as_ref();
        assert_eq!(initiator.unwrap().signatures, [vec![0x30, 0x82, 0xaa]]);
        assert_eq!(s.shared_users[0].signatures, android.signatures);
        assert_eq!(
            app.key_set_data.defined_key_sets,
            [(Some("upgrade".into()), 3)]
        );
        assert_eq!(
            app.mime_groups[0].1,
            [Some("image/gif".into()), Some("image/png".into())]
        );
        let sys = &s.disabled_system_packages[0];
        assert_eq!((sys.flags, sys.private_flags), (1, 8));
        assert!(sys.shared_user);
        assert_eq!(
            s.renamed_packages,
            [("org.example.new".into(), "org.example.old".into())]
        );
        assert_eq!(s.key_sets.public_keys, [(1, b"key".to_vec())]);
        assert_eq!(s.key_sets.key_sets, [(1, vec![1])]);
        assert_eq!(s.key_sets.last_issued_key_set_id, 3);

        assert_eq!(state.list[0].gids, [3003]);

        let user = &state.users[0].1;
        let r = &user.restrictions;
        // The unknown package is dropped; a missing first install time is
        // the settings' legacy one.
        assert_eq!(r.packages.len(), 2);
        let (_, android) = &r.packages[0];
        assert!(android.installed && android.ce_data_inode == 5);
        assert_eq!(android.first_install_time, 0x1234);
        let (_, app) = &r.packages[1];
        assert!(app.stopped && app.not_launched);
        assert_eq!((app.enabled, app.install_reason), (3, 4));
        assert_eq!(app.last_disable_app_caller.as_deref(), Some("shell:1000"));
        assert_eq!(app.first_install_time, 0x2b);
        assert_eq!(
            app.disabled_components.as_deref(),
            Some(["org.example.app.B".to_string()].as_slice())
        );
        assert_eq!(r.default_browser.as_deref(), Some("org.example.app"));
        assert_eq!(r.block_uninstall, ["org.example.app"]);

        let access = state.access.as_ref().unwrap();
        // A definition of an unknown type is left out.
        assert_eq!(access.permissions.len(), 2);
        assert_eq!(access.permissions[1].dynamic, Some((5, Some("Dyn".into()))));
        let a = user.access.as_ref().unwrap();
        assert_eq!(a.package_versions, [("org.example.app".into(), 1)]);
        assert_eq!(
            a.default_permission_grant_fingerprint.as_deref(),
            Some("vendor/product:16/b")
        );
        assert_eq!(
            a.app_id_permissions[0].1[1],
            ("android.permission.INTERNET".into(), 3)
        );
        assert_eq!(a.app_id_device_permissions[0].1[0].0, "device:1");
        assert_eq!(a.app_id_app_ops[0].1, [("android:camera".into(), 1)]);
        assert_eq!(a.package_app_ops[0].0, "org.example.app");

        let rp = user.runtime_permissions.as_ref().unwrap();
        assert_eq!(
            (rp.version, rp.fingerprint.as_deref()),
            (0, Some("f1?pc_version=1"))
        );
        let legacy = &rp.packages[0].1;
        assert!(legacy[0].granted && legacy[0].flags == 0x300);
        assert!(!legacy[1].granted);
        assert_eq!(rp.shared_users, [("android.uid.system".into(), Vec::new())]);
    }

    #[test]
    fn chooses_files_as_the_original() {
        let d = data("files", false);
        let system = d.join("system");
        let user = d.join("system/users/0");
        let read = || State::read(&d, &[0]);
        let reserve = |p: &Path| fs::copy(p, sibling(p, ".reservecopy")).unwrap();

        // A main file that does not parse falls back to its reserve copy.
        reserve(&system.join("packages.xml"));
        fs::write(system.join("packages.xml"), "<packages>").unwrap();
        assert_eq!(read().unwrap().unwrap().settings.packages.len(), 2);
        // Without one, the error is reported.
        fs::remove_file(sibling(&system.join("packages.xml"), ".reservecopy")).unwrap();
        assert!(read().is_err());
        // A write's backup wins over both.
        fs::write(system.join("packages-backup.xml"), "<packages />").unwrap();
        assert!(read().unwrap().unwrap().settings.packages.is_empty());

        // Without settings, the boot is a first boot.
        fs::remove_file(system.join("packages-backup.xml")).unwrap();
        fs::remove_file(system.join("packages.xml")).unwrap();
        assert_eq!(read().unwrap(), None);

        // Without a user's package state, every package is installed and
        // enabled for them.
        fs::write(system.join("packages.xml"), PACKAGES).unwrap();
        fs::remove_file(user.join("package-restrictions.xml")).unwrap();
        let state = read().unwrap().unwrap();
        let r = &state.users[0].1.restrictions;
        assert_eq!(r.packages.len(), 2);
        assert!(
            r.packages
                .iter()
                .all(|(_, s)| *s == UserState::initialized())
        );

        // A missing access file is missing even with a reserve copy: the
        // original then migrates.
        let access = d.join("misc/apexdata/com.android.permission/access.abx");
        reserve(&access);
        fs::remove_file(&access).unwrap();
        assert_eq!(read().unwrap().unwrap().access, None);
        fs::remove_dir_all(d).unwrap();
    }
}

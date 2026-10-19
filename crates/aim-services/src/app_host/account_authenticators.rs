//! `AccountAuthenticatorCache` and the `RegisteredServicesCache` it is
//! (`android-16.0.0_r1`): a user's services for
//! `android.accounts.AccountAuthenticator`, each described by the
//! `<account-authenticator>` its meta-data names, one per account type.
//! The uid that first registered a type keeps it, unless an app of the
//! system image claims it or the first one is gone, as
//! `registered_services/android.accounts.AccountAuthenticator.xml` in the
//! user's system directory persists. Packages do not change while the
//! app host runs, so a user's services are generated once
//! (`updateServices` would find nothing to do) and nobody is told of a
//! change: AccountManagerService validates a user's accounts against them
//! when it opens them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use aim_android_xml::{Element, Node, Value as XmlValue};
use aim_binder_host::parcel::{
    EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::AppHost;
use crate::package::component_resolver::Info;
use crate::package::info::flags::{
    GET_META_DATA, MATCH_DIRECT_BOOT_AWARE, MATCH_DIRECT_BOOT_UNAWARE,
};
use crate::package::intent::Intent;
use crate::package::pkg::Value;

/// `AccountManager.ACTION_AUTHENTICATOR_INTENT`,
/// `AUTHENTICATOR_META_DATA_NAME`, `AUTHENTICATOR_ATTRIBUTES_NAME`.
const ACTION: &str = "android.accounts.AccountAuthenticator";
const META_DATA: &str = "android.accounts.AccountAuthenticator";
const ATTRIBUTES: &str = "account-authenticator";

/// `AuthenticatorDescription`.
#[derive(Clone, Debug, PartialEq)]
pub struct AuthenticatorDescription {
    pub kind: String,
    pub package: String,
    pub label_id: i32,
    pub icon_id: i32,
    pub small_icon_id: i32,
    pub account_preferences_id: i32,
    pub custom_tokens: bool,
}

impl WriteParcelable for AuthenticatorDescription {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.kind));
        p.write_string16(Some(&self.package));
        p.write_i32(self.label_id);
        p.write_i32(self.icon_id);
        p.write_i32(self.small_icon_id);
        p.write_i32(self.account_preferences_id);
        // `writeByte`: an int.
        p.write_i32(self.custom_tokens as i32);
    }
}

impl ReadParcelable for AuthenticatorDescription {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(AuthenticatorDescription {
            kind: r.read_string16()?.unwrap_or_default(),
            package: r.read_string16()?.unwrap_or_default(),
            label_id: r.read_i32()?,
            icon_id: r.read_i32()?,
            small_icon_id: r.read_i32()?,
            account_preferences_id: r.read_i32()?,
            custom_tokens: r.read_i32()? == 1,
        })
    }
}

/// `RegisteredServicesCache.ServiceInfo<AuthenticatorDescription>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Authenticator {
    pub description: AuthenticatorDescription,
    pub uid: i32,
    /// Its service's class.
    pub class: String,
}

/// `generateServicesMap(null, user)`: `user`'s authenticators, the
/// persisted services updated (and written) as they changed.
pub fn generate(host: &AppHost, user: i32) -> Result<Vec<Authenticator>, Exception> {
    let (state, resolution) = host.packages()?;
    let flags = GET_META_DATA | MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE;
    let intent = Intent {
        action: Some(ACTION.into()),
        ..Default::default()
    };
    let found = resolution
        .query_intent_services(&intent, None, flags, user, 1000)
        .map_err(|e| {
            Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("queryIntentServices: {e:?}"),
            )
        })?;
    let inputs = host.inputs()?;
    let root = host.root()?;
    let mut services = Vec::new();
    for r in found {
        let Info::Service(si) = r.info else { continue };
        let package = si.info.item.package_name.clone().unwrap_or_default();
        let name = si.info.item.name.clone().unwrap_or_default();
        // `loadXmlMetaData`: the meta-data's resource, else none.
        let id = si
            .info
            .item
            .meta_data
            .as_ref()
            .and_then(|m| m.0.iter().find(|(k, _)| k == META_DATA))
            .and_then(|(_, v)| match v {
                Value::Int(id) => Some(*id as u32),
                _ => None,
            })
            .filter(|id| *id != 0);
        let pkg = state.packages.get(&package).and_then(|ps| ps.pkg.clone());
        let parsed = match (id, pkg) {
            (Some(id), Some(pkg)) => {
                inputs
                    .packages
                    .xml_resource(root, &inputs.data, &pkg, id, |e, ctx| {
                        if e.name != ATTRIBUTES {
                            return Err(format!("Meta-data does not start with {ATTRIBUTES} tag"));
                        }
                        // `parseServiceAttributes`.
                        let a = ctx.obtain(e);
                        let kind = a.string("accountType").filter(|t| !t.is_empty());
                        Ok(kind.map(|kind| AuthenticatorDescription {
                            kind,
                            package: package.clone(),
                            label_id: a.resource_id("label", 0),
                            icon_id: a.resource_id("icon", 0),
                            small_icon_id: a.resource_id("smallIcon", 0),
                            account_preferences_id: a.resource_id("accountPreferences", 0),
                            custom_tokens: a.boolean("customTokens", false),
                        }))
                    })
            }
            (None, _) => Ok(Err(format!("No {META_DATA} meta-data"))),
            (_, None) => Ok(Err(format!(
                "Unable to load resources for package {package}"
            ))),
        };
        match parsed.and_then(|r| r) {
            Ok(Some(description)) => services.push(Authenticator {
                description,
                uid: si.info.application_info.uid,
                class: si.info.item.name.clone().unwrap_or_default(),
            }),
            Ok(None) => {
                eprintln!("app host: accounts: unable to load service info {package}/{name}")
            }
            Err(e) => {
                eprintln!("app host: accounts: unable to load service info {package}/{name}: {e}")
            }
        }
    }
    let path = inputs.data.join(format!(
        "system/users/{user}/registered_services/{ACTION}.xml"
    ));
    let mut persisted = read_persisted(&path);
    let query = crate::package::query::Query {
        state: &state,
        filter: &resolution.apps_filter,
        calling_uid: 1000,
    };
    // `inSystemImage`: a package of the uid is the system image's.
    let in_system_image = |uid: i32| -> Result<bool, Exception> {
        let packages = query.packages_for_uid(uid).map_err(|e| {
            Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("getPackagesForUid: {}", e.0),
            )
        })?;
        Ok(packages
            .into_iter()
            .flatten()
            .flatten()
            .any(|name| state.packages.get(&name).is_some_and(|ps| ps.is.system)))
    };
    let (services, changed) = merge(services, &mut persisted, in_system_image)?;
    if changed && let Err(e) = write_persisted(&path, &persisted) {
        eprintln!("app host: accounts: error writing {}: {e}", path.display());
    }
    Ok(services)
}

/// The services `found` merged with the `persisted` types' uids, as
/// `generateServicesMap` merges them: whether the persisted ones changed.
fn merge(
    found: Vec<Authenticator>,
    persisted: &mut Vec<(String, i32)>,
    in_system_image: impl Fn(i32) -> Result<bool, Exception>,
) -> Result<(Vec<Authenticator>, bool), Exception> {
    let mut services: Vec<Authenticator> = Vec::new();
    let mut changed = false;
    let put = |services: &mut Vec<Authenticator>, info: &Authenticator| match services
        .iter_mut()
        .find(|s| s.description.kind == info.description.kind)
    {
        Some(s) => *s = info.clone(),
        None => services.push(info.clone()),
    };
    for info in &found {
        let kind = &info.description.kind;
        match persisted.iter().position(|(t, _)| t == kind) {
            None => {
                changed = true;
                put(&mut services, info);
                persisted.push((kind.clone(), info.uid));
            }
            Some(i) if persisted[i].1 == info.uid => put(&mut services, info),
            Some(i) => {
                let previous = persisted[i].1;
                let previous_found = found
                    .iter()
                    .any(|f| f.description.kind == *kind && f.uid == previous);
                if in_system_image(info.uid)? || !previous_found {
                    changed = true;
                    put(&mut services, info);
                    persisted[i].1 = info.uid;
                }
            }
        }
    }
    // A persisted type no service has any more is removed (every uid
    // counts as changed in a full generation).
    let before = persisted.len();
    persisted.retain(|(kind, _)| found.iter().any(|f| f.description.kind == *kind));
    changed |= persisted.len() != before;
    Ok((services, changed))
}

/// `readPersistentServicesLocked`: the types and their uids; none without
/// the file, and those before an error, which is logged.
fn read_persisted(path: &Path) -> Vec<(String, i32)> {
    let mut services = Vec::new();
    let mut read = || -> Result<(), String> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        };
        let root = aim_android_xml::read(&bytes)?;
        if root.name != "services" {
            return Ok(());
        }
        for service in root.children().filter(|e| e.name == "service") {
            let kind = service.string("type").ok_or("type cannot be null")?;
            let uid = service.int("uid")?.ok_or("no uid")?;
            services.push((kind.into_owned(), uid));
        }
        Ok(())
    };
    if let Err(e) = read() {
        eprintln!("app host: accounts: error reading {}: {e}", path.display());
    }
    services
}

/// `writePersistentServicesLocked`, as `AtomicFile` writes the file.
fn write_persisted(path: &Path, services: &[(String, i32)]) -> std::io::Result<()> {
    let root = Element {
        name: "services".into(),
        attrs: Vec::new(),
        content: services
            .iter()
            .map(|(kind, uid)| {
                Node::Element(Element {
                    name: "service".into(),
                    attrs: vec![
                        ("uid".into(), XmlValue::Int(*uid)),
                        ("type".into(), XmlValue::String(kind.clone())),
                    ],
                    content: Vec::new(),
                })
            })
            .collect(),
    };
    let bytes = aim_android_xml::abx::write(&root).map_err(std::io::Error::other)?;
    let new = path.with_extension("xml.new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authenticator(kind: &str, uid: i32) -> Authenticator {
        Authenticator {
            description: AuthenticatorDescription {
                kind: kind.into(),
                package: format!("pkg.{uid}"),
                label_id: 0x7f01_0001,
                icon_id: 0,
                small_icon_id: 0,
                account_preferences_id: 0,
                custom_tokens: true,
            },
            uid,
            class: "Authenticator".into(),
        }
    }

    /// `AuthenticatorDescription.writeToParcel`: two strings, four ints,
    /// a byte.
    #[test]
    fn description_parcels_as_the_original() {
        let a = authenticator("com.google", 10144).description;
        let mut p = Parcel::new();
        a.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("com.google"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("pkg.10144"));
        assert_eq!(r.read_i32().unwrap(), 0x7f01_0001);
        for _ in 0..3 {
            assert_eq!(r.read_i32().unwrap(), 0);
        }
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.remaining(), 0);
        assert_eq!(
            AuthenticatorDescription::read_from(&mut p.reader()).unwrap(),
            a
        );
    }

    /// `generateServicesMap`'s four cases and removals.
    #[test]
    fn the_first_uid_keeps_a_type() {
        let no_system = |_| Ok(false);
        // New types are added and persisted.
        let mut persisted = Vec::new();
        let (services, changed) =
            merge(vec![authenticator("a", 10001)], &mut persisted, no_system).unwrap();
        assert!(changed);
        assert_eq!(services, vec![authenticator("a", 10001)]);
        assert_eq!(persisted, vec![("a".into(), 10001)]);
        // The same uid: unchanged.
        let (_, changed) =
            merge(vec![authenticator("a", 10001)], &mut persisted, no_system).unwrap();
        assert!(!changed);
        // Another uid while the first is still there: ignored.
        let found = vec![authenticator("a", 10001), authenticator("a", 10002)];
        let (services, changed) = merge(found.clone(), &mut persisted, no_system).unwrap();
        assert!(!changed);
        assert_eq!(services, vec![authenticator("a", 10001)]);
        // ...unless it is the system image's.
        let (services, changed) = merge(found, &mut persisted, |uid| Ok(uid == 10002)).unwrap();
        assert!(changed);
        assert_eq!(services, vec![authenticator("a", 10002)]);
        assert_eq!(persisted, vec![("a".into(), 10002)]);
        // Another uid once the first is gone replaces it.
        let (services, changed) =
            merge(vec![authenticator("a", 10003)], &mut persisted, no_system).unwrap();
        assert!(changed);
        assert_eq!(services, vec![authenticator("a", 10003)]);
        // A type no service has any more is removed.
        let (services, changed) =
            merge(vec![authenticator("b", 10004)], &mut persisted, no_system).unwrap();
        assert!(changed);
        assert_eq!(services, vec![authenticator("b", 10004)]);
        assert_eq!(persisted, vec![("b".into(), 10004)]);
    }

    /// The persisted services as `writePersistentServicesLocked` writes
    /// them read back.
    #[test]
    fn persisted_services_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "aim-registered-services-{}.xml",
            std::process::id()
        ));
        let services = vec![
            ("com.google".to_string(), 10144),
            ("com.google.android.gm.pop3".to_string(), 10156),
        ];
        write_persisted(&path, &services).unwrap();
        assert_eq!(read_persisted(&path), services);
        std::fs::remove_file(&path).unwrap();
        assert!(read_persisted(&path).is_empty());
    }
}

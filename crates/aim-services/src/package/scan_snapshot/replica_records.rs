//! Every record a replica assembles each package and shared UID from, written
//! at version 0, so a replica refetches only the owners whose records changed
//! between two captures of one lineage (#1400).
use super::{Snapshot, endpoint, runtime_record, setting_record, shared_record, user_record};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
use std::collections::BTreeMap;

pub struct Records {
    packages: BTreeMap<(String, bool), Package>,
    shared: BTreeMap<String, Vec<u8>>,
}

impl std::fmt::Debug for Records {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Records")
            .field("packages", &self.packages.len())
            .field("shared", &self.shared.len())
            .finish()
    }
}

struct Package {
    records: Vec<Vec<u8>>,
    code: Option<endpoint::PackageCode>,
}

impl PartialEq for Package {
    fn eq(&self, other: &Self) -> bool {
        self.records == other.records
            && match (&self.code, &other.code) {
                (None, None) => true,
                (Some(code), Some(other)) => code.same_record(other),
                _ => false,
            }
    }
}

/// Owners present in a capture whose records differ from an earlier one's.
pub struct Changes<'a> {
    pub packages: Vec<(&'a str, bool)>,
    pub shared: Vec<&'a str>,
}

impl Records {
    pub(super) fn capture(snapshot: &Snapshot) -> Result<Self, String> {
        let owner = snapshot.owner();
        let mut packages = BTreeMap::new();
        for (settings, factory) in [
            (&owner.settings.packages, false),
            (&owner.settings.disabled_system_packages, true),
        ] {
            for setting in settings {
                let name = setting.name.as_str();
                packages.insert((name.to_owned(), factory), package(snapshot, name, factory)?);
            }
        }
        let mut shared = BTreeMap::new();
        for name in owner.identities.shared_users.keys() {
            let record = shared_record::captured_owner(0, owner, name)?
                .ok_or_else(|| format!("missing shared UID record: {name}"))?;
            shared.insert(name.clone(), record);
        }
        Ok(Self { packages, shared })
    }

    pub fn changed_since(&self, before: &Self) -> Changes<'_> {
        Changes {
            packages: self
                .packages
                .iter()
                .filter(|(key, package)| before.packages.get(*key) != Some(*package))
                .map(|((name, factory), _)| (name.as_str(), *factory))
                .collect(),
            shared: self
                .shared
                .iter()
                .filter(|(name, record)| before.shared.get(*name) != Some(*record))
                .map(|(name, _)| name.as_str())
                .collect(),
        }
    }
}

fn package(snapshot: &Snapshot, name: &str, factory: bool) -> Result<Package, String> {
    let missing = || format!("missing replica record: {name}");
    let written = |record: &dyn WriteParcelable| {
        let mut parcel = Parcel::new();
        record.write_to(&mut parcel);
        parcel.data().to_vec()
    };
    let mut signing = endpoint::PackageSigningState::captured(snapshot, name, factory)?
        .ok_or_else(missing)?;
    signing.version = 0;
    let mut transient =
        endpoint::PackageTransientState::captured(snapshot, name, factory).ok_or_else(missing)?;
    transient.version = 0;
    let policy = snapshot
        .owner()
        .hidden_api_enforcement_policy(name, factory)?
        .ok_or_else(missing)?;
    // Active usage times reach a replica through the usage records.
    let mut records = vec![
        setting_record::captured_at(snapshot, 0, name, factory)?.ok_or_else(missing)?,
        runtime_record::captured_at(snapshot, 0, name, factory, !factory)?.ok_or_else(missing)?,
        written(&signing),
        written(&transient),
        policy.to_le_bytes().to_vec(),
    ];
    for user in user_record::ids(snapshot, name, factory)?.ok_or_else(missing)? {
        records.push(
            user_record::captured_at(snapshot, 0, name, factory, user)?.ok_or_else(missing)?,
        );
    }
    Ok(Package {
        records,
        code: endpoint::PackageCode::captured(snapshot, name, factory)?,
    })
}

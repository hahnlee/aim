//! Actual successful package registration facts for original permission initialization.
use super::SigningScan;
use crate::package::pkg::AndroidPackage;
use aim_binder_host::parcel::Parcel;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Admission {
    pub name: String,
    pub scan_as_instant: bool,
    pub old_package: Option<AndroidPackage>,
}
impl SigningScan {
    pub fn permission_admissions(&self) -> Result<Vec<&Admission>, String> {
        let registry = self.package_registry()?;
        ordered_admissions(
            &registry.ordered_package_names(),
            &self.permission_admissions,
        )
    }

    /// The current registered view replaces earlier admissions of the same name;
    /// the last receipt still owns its actual pre-replacement package.
    pub fn permission_admissions_record(&self) -> Result<Vec<u8>, String> {
        let registry = self.package_registry()?;
        let names = registry.ordered_package_names();
        let admissions = self.permission_admissions()?;
        let mut parcel = Parcel::new();
        parcel.write_i32(1);
        parcel.write_i32(
            i32::try_from(names.len()).map_err(|_| "permission admission inventory too large")?,
        );
        for receipt in admissions {
            parcel.write_string16(Some(&receipt.name));
            parcel.write_bool(receipt.scan_as_instant);
            let cache = receipt
                .old_package
                .as_ref()
                .map(|package| package.to_cache_entry().map(|cache| cache.bytes))
                .transpose()?;
            aim_service_aidl::write_byte_array(&mut parcel, cache.as_deref());
        }
        Ok(parcel.data().to_vec())
    }
}

/// ScanPackageUtils.adjustScanFlagsWithPackageSetting: a null boot user is USER_SYSTEM.
/// An absent sparse user row uses PackageUserStateDefault.instantApp=false.
pub fn boot_scan_as_instant(
    users: Option<&BTreeMap<String, BTreeMap<i32, crate::package::restrictions::UserState>>>,
    name: &str,
) -> bool {
    users
        .and_then(|users| users.get(name))
        .and_then(|users| users.get(&0))
        .is_some_and(|user| user.instant_app)
}

fn ordered_admissions<'a>(
    names: &[&str],
    history: &'a [Admission],
) -> Result<Vec<&'a Admission>, String> {
    let latest = history
        .iter()
        .enumerate()
        .map(|(index, receipt)| (receipt.name.as_str(), (index, receipt)))
        .collect::<BTreeMap<_, _>>();
    let mut selected = names
        .iter()
        .map(|name| {
            latest.get(name).copied().ok_or_else(|| {
                format!("registered package has no committed permission admission: {name}")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    // PermissionManager's first system declaration wins. Replay real committed
    // scan events, retaining the last replacement occurrence, not ArrayMap order.
    selected.sort_by_key(|(index, _)| *index);
    Ok(selected.into_iter().map(|(_, receipt)| receipt).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn admission(name: &str, instant: bool) -> Admission {
        Admission {
            name: name.into(),
            scan_as_instant: instant,
            old_package: None,
        }
    }
    #[test]
    fn system_declaration_replay_preserves_both_committed_scan_orders() {
        let registry = ["a.system", "z.system"];
        // Either first-system-declaration outcome is valid, depending on the
        // real scan completion order. Registry hash/name iteration is not that order.
        for order in [["z.system", "a.system"], ["a.system", "z.system"]] {
            let history = order
                .iter()
                .map(|name| admission(name, false))
                .collect::<Vec<_>>();
            let replay = ordered_admissions(&registry, &history).unwrap();
            assert_eq!(
                replay
                    .iter()
                    .map(|receipt| receipt.name.as_str())
                    .collect::<Vec<_>>(),
                order
            );
            assert!(std::ptr::eq(replay[0], &history[0]));
        }
    }
    #[test]
    fn latest_replacement_keeps_its_event_slot_and_missing_registered_receipt_fails() {
        let history = vec![
            admission("z.system", false),
            admission("a.system", false),
            admission("retired", false),
            admission("z.system", true),
        ];
        let replay = ordered_admissions(&["a.system", "z.system"], &history).unwrap();
        assert!(std::ptr::eq(replay[0], &history[1]));
        assert!(std::ptr::eq(replay[1], &history[3]));
        assert!(replay[1].scan_as_instant);
        assert!(
            ordered_admissions(&["missing"], &history)
                .unwrap_err()
                .contains("no committed permission admission")
        );
    }
}

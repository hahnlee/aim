//! Settings boot records share one UID owner and one per-attempt table.
use super::{Package, PackageReadAttempt, ReadError, Settings, SharedUser};
use crate::package::owner::app_ids::AppIds;
use aim_android_xml::{Element, pull::Reader};
use std::collections::BTreeMap;

/// Owners whose state is outside the persisted Settings record. Implementations
/// must report missing dependencies; recovery only retries File errors.
pub trait ReadOwners {
    fn package_child(
        &mut self,
        package: &mut Package,
        reader: &mut Reader<'_>,
        start: &Element,
    ) -> Result<bool, ReadError>;
    fn shared_child(
        &mut self,
        group: &mut SharedUser,
        reader: &mut Reader<'_>,
        start: &Element,
    ) -> Result<bool, ReadError>;
    fn public_key(&mut self, encoded: &[u8]) -> Result<Option<Vec<u8>>, ReadError>;
    fn finish_key_sets(
        &mut self,
        settings: &mut Settings,
        refs: &BTreeMap<i64, i32>,
    ) -> Result<(), ReadError>;
    fn global_record(
        &mut self,
        settings: &mut Settings,
        reader: &mut Reader<'_>,
        start: &Element,
    ) -> Result<bool, ReadError>;
}

impl Settings {
    /// Each recursive recovery attempt resets transient tables while retaining
    /// already registered Settings and UID owners. Pending binding and related
    /// user files follow this read; their inputs remain available in `attempt`.
    pub fn read_owned_document(
        &mut self,
        bytes: &[u8],
        ids: &mut AppIds,
        attempt: &mut PackageReadAttempt,
        owners: &mut impl ReadOwners,
    ) -> Result<Option<Element>, ReadError> {
        *attempt = PackageReadAttempt::default();
        self.read_document(bytes, |settings, reader, start| {
            match start.name.as_str() {
                "package" => {
                    settings.read_package(
                        reader,
                        start,
                        ids,
                        attempt,
                        |package, reader, child| {
                            let handled = owners.package_child(package, reader, child)?;
                            require_child_owner(handled, child, &["perms", "domain-verification"])
                        },
                    )?;
                }
                "shared-user" => {
                    settings.read_shared_user(
                        reader,
                        start,
                        ids,
                        attempt,
                        |group, reader, child| {
                            let handled = owners.shared_child(group, reader, child)?;
                            require_child_owner(handled, child, &["perms"])
                        },
                    )?;
                }
                "keyset-settings" => {
                    // Both callbacks mutate the same external owner in sequence.
                    let owners = std::cell::RefCell::new(&mut *owners);
                    settings.read_key_sets(
                        reader,
                        start,
                        &attempt.key_set_refs,
                        |bytes| owners.borrow_mut().public_key(bytes),
                        |settings, refs| owners.borrow_mut().finish_key_sets(settings, refs),
                    )?;
                }
                _ => return owners.global_record(settings, reader, start),
            }
            Ok(true)
        })
    }
}

fn require_child_owner(
    handled: bool,
    child: &Element,
    required: &[&str],
) -> Result<bool, ReadError> {
    if !handled && required.contains(&child.name.as_str()) {
        return Err(ReadError::Owner(format!(
            "settings child owner unavailable: {}",
            child.name
        )));
    }
    Ok(handled)
}

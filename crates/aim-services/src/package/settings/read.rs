//! Settings boot records share one UID owner and one per-attempt table.
use super::{Package, PackageReadAttempt, ReadError, Settings, SharedUser};
use crate::package::owner::app_ids::AppIds;
use aim_android_xml::{Element, pull::Reader};

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
        domain_uuid_strict_validation: bool,
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
                    settings.read_key_sets(
                        reader,
                        start,
                        &attempt.key_set_refs,
                        |bytes| owners.public_key(bytes),
                        crate::package::owner::key_sets::finish_read,
                    )?;
                }
                "domain-verifications" => {
                    settings.read_boot_domains(reader, domain_uuid_strict_validation)?;
                }
                _ => return owners.global_record(settings, reader, start),
            }
            Ok(true)
        })
    }

    /// Constructor-time domain reads precede package attachment. Keep pending
    /// maps across retries, and publish a container only after UUID decoding.
    pub fn read_boot_domains(
        &mut self,
        reader: &mut Reader<'_>,
        strict: bool,
    ) -> Result<(), ReadError> {
        use crate::package::domain_verification::{State, owner::Owner, uuid};
        let result = State::read_events(reader, |id| {
            uuid::parse(id, strict).map_err(ReadError::File)
        })?;
        let mut owner = Owner::new(
            self.domain_verification.clone(),
            self.legacy_domain_info.clone(),
        );
        let diagnostics = owner
            .read_settings(result, |_| {
                Err("boot persistence read unexpectedly requires attached domain code".into())
            })
            .map_err(ReadError::Owner)?;
        self.domain_verification = owner.persisted();
        for error in diagnostics {
            eprintln!(
                "domain SettingsXml depth {}: {}",
                error.depth, error.message
            );
        }
        Ok(())
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

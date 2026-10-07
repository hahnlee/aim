//! Settings boot records share one UID owner and one per-attempt table.
use super::{Package, PackageReadAttempt, ReadError, Settings, SharedUser};
use crate::package::owner::app_ids::AppIds;
use aim_android_xml::{Element, pull::Reader};

/// Owners whose state is outside the persisted Settings record. Implementations
/// must report missing dependencies; recovery only retries File errors.
pub trait ReadOwners {
    fn factory_record(
        &mut self,
        settings: &mut Settings,
        reader: &mut Reader<'_>,
        start: &Element,
        ids: &AppIds,
    ) -> Result<(), ReadError>;
    fn start_attempt(&mut self, settings: &Settings, pending: &[Package]) -> Result<(), ReadError>;
    fn package_registered(&mut self, package: &Package, created: bool) -> Result<(), ReadError>;
    fn shared_registered(&mut self, group: &SharedUser, created: bool) -> Result<(), ReadError>;
    fn package_child(
        &mut self,
        package: &mut Package,
        reader: &mut Reader<'_>,
        start: &Element,
        ids: &AppIds,
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
    /// PMS creates its platform/OEM shared UID owners before any settings read.
    /// Keep constructor state on the caller so an owner failure retains effects.
    pub fn initialize_shared_bootstrap(
        &mut self,
        boot: &crate::package::owner::shared_users::Bootstrap,
        ids: &mut AppIds,
        owners: &mut impl ReadOwners,
    ) -> Result<(), ReadError> {
        if !self.packages.is_empty() || !self.shared_users.is_empty() || *ids != AppIds::default() {
            return Err(ReadError::Owner(
                "shared UID initialization requires a fresh settings owner".into(),
            ));
        }
        let groups = boot.ordered_shared_users().map_err(ReadError::Owner)?;
        for (name, source) in groups {
            ids.register_existing(
                source.app_id,
                crate::package::owner::app_ids::Owner::SharedUser(name.into()),
            )
            .map_err(|error| ReadError::Owner(format!("shared UID initialization: {error:?}")))?;
            self.shared_users.push(SharedUser {
                name: name.into(),
                app_id: source.app_id,
                flags: source.flags,
                signatures: source.signatures.clone(),
            });
            owners.shared_registered(self.shared_users.last().unwrap(), true)?;
        }
        Ok(())
    }

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
        owners.start_attempt(self, &attempt.pending)?;
        *attempt = PackageReadAttempt::default();
        self.read_document(bytes, |settings, reader, start| {
            match start.name.as_str() {
                "updated-package" => owners.factory_record(settings, reader, &start, ids)?,
                "package" => {
                    settings.read_package_with_ids(
                        reader,
                        start,
                        ids,
                        attempt,
                        |package, reader, child, ids, created| {
                            let Some(child) = child else {
                                owners.package_registered(package, created)?;
                                return Ok(true);
                            };
                            let handled = owners.package_child(package, reader, child, ids)?;
                            require_child_owner(handled, child, &["perms", "domain-verification"])
                        },
                    )?;
                }
                "shared-user" => {
                    let existing = start
                        .string("name")
                        .is_some_and(|name| settings.shared_users.iter().any(|g| g.name == name));
                    settings.read_shared_user_with_owner(
                        reader,
                        start,
                        ids,
                        attempt,
                        |group, reader, child| {
                            let Some(child) = child else {
                                owners.shared_registered(group, !existing)?;
                                return Ok(true);
                            };
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

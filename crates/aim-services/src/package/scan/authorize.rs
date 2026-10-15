//! Normal verifySignatures gates for persisted active APK scan inputs.
//! APK integrity is already verified. Compatibility/recovery, upgrade
//! keysets and owner-authorized rollback require reconciliation (#804).
use super::{settings, sign};
use sign::{History, JoinType};

pub(super) fn saved(
    package: &settings::Package,
    verified: &sign::SigningDetails,
    settings: &settings::Settings,
) -> Result<(), String> {
    with_disabled(
        package,
        verified,
        settings,
        settings
            .disabled_system_packages
            .iter()
            .find(|p| p.name == package.name),
    )
}

/// The disabled setting belongs to the scanned request, even when a static
/// library's signature-check setting belongs to a different version.
pub(super) fn with_disabled(
    package: &settings::Package,
    verified: &sign::SigningDetails,
    settings: &settings::Settings,
    disabled: Option<&settings::Package>,
) -> Result<(), String> {
    let candidate = History::verified(verified);
    let known = |s: &settings::Signatures| !s.signatures.is_empty();
    let previous = package.signatures.as_ref().filter(|s| known(s));
    if let Some(previous) = previous {
        if !candidate.allows_update_from(&History::saved(previous), false) {
            return Err(
                "existing package signatures mismatch (INSTALL_FAILED_UPDATE_INCOMPATIBLE; #804)"
                    .into(),
            );
        }
        if let Some(original) = disabled
            .and_then(|p| p.signatures.as_ref())
            .filter(|s| known(s))
            && !candidate.allows_update_from(&History::saved(original), false)
        {
            return Err("updated system package signatures mismatch (INSTALL_FAILED_UPDATE_INCOMPATIBLE; #804)".into());
        }
    }
    if package.shared_user {
        let group = settings
            .shared_users
            .iter()
            .find(|g| Some(g.app_id) == package.shared_app_id())
            .ok_or_else(|| format!("missing shared UID group {} (#803)", package.app_id))?;
        if let Some(details) = group.signatures.as_ref().filter(|s| known(s)) {
            let unknown = settings::Signatures::default();
            let members: Vec<_> = settings
                .packages
                .iter()
                .filter(|p| p.shared_app_id() == Some(group.app_id))
                .map(|p| History::saved(p.signatures.as_ref().unwrap_or(&unknown)))
                .collect();
            let group_history = History::saved(details);
            let kind = if previous.is_some() {
                JoinType::Update
            } else {
                JoinType::Install
            };
            if !candidate.can_join_shared_user(&group_history, kind, &members) {
                return Err(format!(
                    "shared UID {} signatures mismatch (INSTALL_FAILED_SHARED_USER_INCOMPATIBLE; #804)",
                    group.name
                ));
            }
            if !candidate.has_common_ancestor(&group_history) {
                return Err(format!(
                    "shared UID {} signing lineages diverge (INSTALL_FAILED_SHARED_USER_INCOMPATIBLE)",
                    group.name
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn certificates(current: u8, past: &[(u8, i32)]) -> settings::Signatures {
        settings::Signatures {
            signatures: vec![vec![current]],
            past_signatures: (!past.is_empty())
                .then(|| past.iter().map(|(c, f)| (vec![*c], *f)).collect()),
            ..Default::default()
        }
    }
    fn verified(details: &settings::Signatures) -> sign::SigningDetails {
        sign::SigningDetails {
            unknown: false,
            current_flags: Vec::new(),
            signatures: details.signatures.clone(),
            scheme_version: 3,
            public_keys: Some(vec![]),
            past_signing_certificates: details.past_signatures.clone(),
        }
    }
    fn setting(details: Option<settings::Signatures>) -> settings::Package {
        settings::Package {
            name: "pkg".into(),
            app_id: 10001,
            signatures: details,
            ..Default::default()
        }
    }

    #[test]
    fn saved_signatures_and_known_disabled_original_both_authorize_code_reuse() {
        let original = certificates(1, &[]);
        let rotated = certificates(2, &[(1, 1), (2, 0)]);
        let package = setting(Some(original.clone()));
        let mut state = settings::Settings::default();
        assert!(saved(&package, &verified(&rotated), &state).is_ok());
        assert!(
            saved(&package, &verified(&certificates(3, &[])), &state)
                .unwrap_err()
                .contains("existing package")
        );
        state
            .disabled_system_packages
            .push(setting(Some(certificates(3, &[]))));
        assert!(
            saved(&package, &verified(&rotated), &state)
                .unwrap_err()
                .contains("updated system")
        );
        state.disabled_system_packages[0].signatures = None;
        assert!(saved(&package, &verified(&rotated), &state).is_ok());
        state.disabled_system_packages[0].signatures = Some(Default::default());
        assert!(saved(&package, &verified(&rotated), &state).is_ok());
    }

    #[test]
    fn disabled_signer_belongs_to_the_request_not_the_selected_library_version() {
        let package = setting(Some(certificates(1, &[])));
        let state = settings::Settings {
            disabled_system_packages: vec![setting(Some(certificates(2, &[])))],
            ..Default::default()
        };
        let candidate = verified(&certificates(1, &[]));
        assert!(saved(&package, &candidate, &state).is_err());
        assert!(with_disabled(&package, &candidate, &state, None).is_ok());
        let request_disabled = settings::Package {
            name: "other_version".into(),
            signatures: Some(certificates(2, &[])),
            ..Default::default()
        };
        assert!(with_disabled(&package, &candidate, &state, Some(&request_disabled)).is_err());
    }

    #[test]
    fn shared_current_signer_does_not_authorize_divergent_ancestors_or_revoked_members() {
        let details = certificates(3, &[(1, 3), (2, 2), (3, 0)]);
        let mut package = setting(Some(details.clone()));
        package.shared_user = true;
        let mut state = settings::Settings {
            packages: vec![package.clone()],
            shared_users: vec![settings::SharedUser {
                name: "group".into(),
                app_id: 10001,
                signatures: Some(certificates(3, &[(4, 3), (2, 2), (3, 0)])),
                ..Default::default()
            }],
            ..Default::default()
        };
        let before = state.clone();
        assert!(
            saved(&package, &verified(&details), &state)
                .unwrap_err()
                .contains("lineages diverge")
        );
        assert_eq!(state, before);
        state.shared_users[0].signatures = Some(certificates(3, &[(2, 0), (3, 0)]));
        assert!(saved(&package, &verified(&details), &state).is_ok());
        package.signatures = None;
        state.packages[0] = package.clone();
        let mut member = setting(Some(certificates(1, &[])));
        member.name = "member".into();
        member.shared_user = true;
        state.packages.push(member);
        state.shared_users[0].signatures = Some(certificates(2, &[]));
        let revoked = certificates(3, &[(1, 1), (2, 2), (3, 0)]);
        assert!(
            saved(&package, &verified(&revoked), &state)
                .unwrap_err()
                .contains("signatures mismatch")
        );
        state.shared_users.clear();
        assert!(
            saved(&package, &verified(&details), &state)
                .unwrap_err()
                .contains("missing shared UID")
        );
    }
}

//! One production preferred initialization entry and concrete UM dependency
//! bindings. Include as a System child module; no original PMS producer is used.
use super::*;
use crate::package::{
    preferred::{
        self, Mutation,
        registry::{ActionError, Actions, Handle},
    },
    query::preferred::Owner,
};
use std::path::Path;
fn preferred_failure(error: ActionError) -> Exception {
    match error {
        ActionError::Exception(error) => error,
        ActionError::Unavailable(error) => {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
        }
    }
}
impl System {
    /// Root constructor calls after native scan/Settings/configured roles and
    /// lifecycle inputs are published, before exposing the PackageManager names.
    pub fn initialize_package_preferred(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        image: &Path,
        config: &crate::package::system_config::SystemConfig,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        // SystemConfig has already been consumed by the native scan and role
        // constructors. Check the same source generation before binding owners.
        let capture = self.capture_package_queries()?;
        let lifecycle = capture.state().system.lifecycle.as_ref().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred lifecycle input unavailable",
            )
        })?;
        let first_boot = lifecycle.first_boot();
        if capture.state().system.system_permissions.as_ref() != Some(&config.system_permissions) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred SystemConfig permission owner differs",
            ));
        }
        if capture.state().system.roles.is_none() || capture.state().system.user_policy.is_none() {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "preferred configured Role/UserManager inputs unavailable",
            ));
        }
        self.install_native_package_preferred(bridge, image)?;
        if first_boot {
            let users: Vec<_> = self
                .capture_package_queries()?
                .state()
                .users
                .keys()
                .copied()
                .collect();
            for user in users {
                self.apply_native_default_preferred(bridge, user)?;
            }
        }
        self.check_package_bootstrap(bridge)
    }

    fn apply_native_default_preferred(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        user: i32,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let handle = capture
            .state()
            .system
            .preferred_owner
            .as_ref()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "preferred source owner unavailable",
                )
            })?;
        let defaults = handle
            .actions
            .default_preferences(user)
            .map_err(preferred_failure)?;
        let snapshot = handle
            .snapshot(user)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "preferred user registry unavailable",
                )
            })?;
        // Initial Settings defaults do not reset permission/network/domain state
        // or enqueue public preference broadcasts, unlike resetApplicationPreferences.
        handle
            .actions
            .commit_mutation(
                user,
                snapshot.generation,
                &Mutation::ApplyDefaults { defaults },
                1000,
            )
            .map_err(preferred_failure)?;
        Ok(())
    }

    /// Settings.systemReady's sweep precedes UserManager.systemReady. Persist
    /// each changed user's actual resolver under the normal publication gate.
    pub(crate) fn sweep_dangling_package_preferred(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let registry = self.package_preferred_registry(bridge)?;
        let users = registry.user_ids().map_err(|error| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
        })?;
        for user in users {
            let capture = self.capture_package_queries()?;
            let components = capture.state().package_registry.as_ref().ok_or_else(|| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "systemReady component registration owner unavailable")
            })?;
            let snapshot = registry.snapshot(user)
                .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "systemReady preferred user owner unavailable"))?;
            let selection = snapshot.state.dangling_selection(|component| components.contains_activity(component));
            if !selection.edits.is_empty() {
                self.commit_package_preferred_selection(bridge, &registry, user,
                    snapshot.generation, &selection)?;
            }
        }
        self.check_package_bootstrap(bridge)
    }

    /// RemovePackageHelper.clearPackageStateForUserLIF, with the target chosen
    /// by the removal owner: USER_ALL only when the package setting is removed.
    pub(crate) fn clear_removed_package_preferred(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        package: &str,
        target_user: i32,
        keep_data: bool,
    ) -> Result<preferred::RemovalEffects> {
        self.check_package_bootstrap(bridge)?;
        if target_user < -1 {
            return Err(Exception::illegal_argument("invalid preferred removal user"));
        }
        let mut effects = preferred::RemovalEffects {
            changed_users: Vec::new(), broadcast_user: target_user,
        };
        if keep_data { return Ok(effects); }
        let registry = self.package_preferred_registry(bridge)?;
        let stages = if target_user == -1 {
            registry.prepare_clear_all(Some(package)).map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
            })?
        } else {
            let Some(snapshot) = registry.snapshot(target_user).map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
            })? else { return Ok(effects); };
            registry.prepare_mutation(target_user, snapshot.generation,
                &Mutation::Clear { package: Some(package.into()) }).map_err(|error| {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
                })?.stage.into_iter().collect()
        };
        for stage in stages {
            let user = stage.user;
            self.commit_package_preferred_stage(bridge, &registry, stage, false)?;
            effects.changed_users.push(user);
        }
        Ok(effects)
    }

    pub(crate) fn finish_removed_package_preferred(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        effects: preferred::RemovalEffects,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        if effects.changed_users.is_empty() { return Ok(()); }
        // Original helper visits changed users in descending SparseBooleanArray
        // order, then sends one notification for the requested removal scope.
        for user in effects.changed_users.into_iter().rev() {
            let capture = self.capture_package_queries()?;
            let handle = capture.state().system.preferred_owner.as_ref().ok_or_else(|| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "removed preferred home owner unavailable")
            })?;
            handle.actions.reconcile_home(user, 1000).map_err(preferred_failure)?;
        }
        bridge.preferred_changed(effects.broadcast_user).map_err(|error| {
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("removed preferred broadcast owner: {error:?}"))
        })?;
        self.check_package_bootstrap(bridge)
    }

    pub fn bind_user_preferred_dependencies(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        dependencies: &mut crate::package::user_operations::Dependencies,
    ) -> Result<()> {
        self.check_package_bootstrap(bridge)?;
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        dependencies.default_preferred = Box::new(move |user| {
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "preferred user system stopped",
                )
            })?;
            system.default_preferred_for_new_user(&retained, user)
        });
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        dependencies.commit_cross_profile_stage = Box::new(move |stage| {
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "cross-profile system stopped",
                )
            })?;
            let registry = system.package_preferred_registry(&retained)?;
            system.commit_package_preferred_stage(&retained, &registry, stage, false)
        });
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        dependencies.add_cross_profile = Box::new(move |filter, owner, source, target, flags| {
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "cross-profile system stopped",
                )
            })?;
            system.check_package_bootstrap(&retained)?;
            let capture = system.capture_package_queries()?;
            let handle = capture
                .state()
                .system
                .preferred_owner
                .as_ref()
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "cross-profile native preferred owner unavailable",
                    )
                })?;
            let level = handle
                .actions
                .cross_access(1000, source, target, true)
                .map_err(preferred_failure)?;
            let owner = owner.ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "null cross-profile owner package",
                )
            })?;
            let snapshot = handle
                .snapshot(source)
                .map_err(|error| {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
                })?
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "cross-profile source user unavailable",
                    )
                })?;
            handle
                .actions
                .commit_mutation(
                    source,
                    snapshot.generation,
                    &Mutation::CrossAdd(preferred::CrossProfileIntentFilter {
                        filter,
                        owner_package: owner,
                        target_user_id: target,
                        flags,
                        access_control: level,
                    }),
                    1000,
                )
                .map_err(preferred_failure)?;
            Ok(())
        });
        Ok(())
    }

    pub(crate) fn default_preferred_for_new_user(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        user: i32,
    ) -> Result<aim_android_xml::Element> {
        self.check_package_bootstrap(bridge)?;
        let registry = self.package_preferred_registry(bridge)?;
        let capture = self.capture_package_queries()?;
        let handle = capture
            .state()
            .system
            .preferred_owner
            .as_ref()
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "new-user preferred owner unavailable",
                )
            })?;
        let defaults = handle
            .actions
            .default_preferences(user)
            .map_err(preferred_failure)?;
        let mut preferred = preferred::Preferred::default();
        for activity in defaults {
            preferred.add_preferred(activity, true);
        }
        if registry
            .snapshot(user)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0))?
            .is_some()
        {
            self.apply_native_default_preferred(bridge, user)?;
            let current = registry
                .snapshot(user)
                .map_err(|error| {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error.0)
                })?
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "new-user preferred generation unavailable",
                    )
                })?;
            let current_capture = registry.capture().map_err(|error| {
                Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
            })?;
            let persistent = current_capture
                .record_order(user, preferred::records::PERSISTENT)
                .map_err(|error| {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
                })?;
            let cross = current_capture
                .record_order(user, preferred::records::CROSS_PROFILE)
                .map_err(|error| {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error)
                })?;
            return Self::native_preferred_sections(
                current.state.as_ref(),
                &current.preferred_order,
                &persistent,
                &cross,
            );
        }
        let stage = registry
            .prepare_new_user(user, preferred)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let sections = Self::native_preferred_sections(
            stage.preferred.as_ref(),
            &stage.preferred_order,
            &stage.persistent_order,
            &stage.cross_profile_order,
        )?;
        // Publish the real typed record family under the same generation gate.
        // UserOperations writes its complete first restrictions document next.
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state
            .current
            .as_mut()
            .filter(|current| {
                Arc::ptr_eq(&current.bridge, bridge)
                    && current
                        .preferred_registry
                        .as_ref()
                        .is_some_and(|owner| Arc::ptr_eq(owner, &registry))
            })
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "new-user preferred bootstrap changed",
                )
            })?;
        let preview = registry
            .preview_new_user(&stage)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        let commit = current.preferred_commit.clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "new-user preferred commit unavailable",
            )
        })?;
        let actions = current.preferred_actions.clone().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "new-user preferred actions unavailable",
            )
        })?;
        let handle = Arc::new(Handle::new(Arc::new(preview), commit, actions));
        let capture = current.queries.as_ref().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "new-user query owner unavailable",
            )
        })?;
        let update = capture
            .prepare_preferred_update(handle, &[])
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        registry
            .publish_new_user(stage)
            .map_err(|error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error))?;
        current.publish_snapshot(update.store);
        current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page {
            page.publish(update.capture.scan().version());
        }
        state.version = update.capture.scan().version();
        Ok(sections)
    }
    fn native_preferred_sections(
        preferred: &preferred::Preferred,
        order: &[usize],
        persistent: &[usize],
        cross: &[usize],
    ) -> Result<aim_android_xml::Element> {
        let mut root = aim_android_xml::Element {
            name: "package-restrictions".into(),
            attrs: Vec::new(),
            content: Vec::new(),
        };
        for section in [
            preferred.preferred_document(order, true),
            preferred.persistent_document(persistent),
            preferred.cross_profile_document(cross),
        ] {
            root.content
                .push(aim_android_xml::Node::Element(section.map_err(
                    |error| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, error),
                )?));
        }
        Ok(root)
    }
}

//! Apply complete original PackageStateMutator results to native owners.
//! No field is a best-effort hint: mismatched identities reject the candidate.
use super::{internal_mutation_record::{Record, Setting, User}, owner::usage::Usage,
    restrictions::UserState, scan::{SigningScan, CapturedUsers, ReplicaRuntime}, scan_snapshot::Snapshot};
use std::collections::{BTreeMap, BTreeSet};

pub struct Applied { pub scan: SigningScan, pub usage: Usage, pub settings_changed: bool,
    pub changed_users: BTreeSet<i32>, pub runtime_changed: bool }
pub fn apply(base: &Snapshot, record: &Record) -> Result<Applied, String> {
    if i64::try_from(base.version()).ok() != Some(record.version) { return Err("internal mutator base version differs".into()); }
    let owner = base.owner();
    let active: BTreeSet<_> = owner.settings.packages.iter().map(|package| package.name.as_str()).collect();
    let disabled: BTreeSet<_> = owner.settings.disabled_system_packages.iter().map(|package| package.name.as_str()).collect();
    if active != record.active.iter().map(|setting| setting.name.as_str()).collect()
        || disabled != record.disabled.iter().map(|setting| setting.name.as_str()).collect()
        || active.len() != record.active.len() || disabled.len() != record.disabled.len() {
        return Err("internal mutator immutable package inventory differs".into());
    }
    let mut scan = owner.clone(); let mut usage = base.usage().clone();
    let mut users = BTreeMap::new(); let mut runtime = BTreeMap::new(); let mut labels = BTreeMap::new();
    let mut changed_users = BTreeSet::new(); let mut runtime_changed = false;
    for (records, factory) in [(&record.active, false), (&record.disabled, true)] {
        for input in records {
            validate_setting(owner, input, factory)?;
            let old_users = if factory { owner.disabled_user_states(&input.name) } else { owner.scanned_user_states(&input.name) }
                .ok_or("internal mutator native user owner absent")?;
            let ids: BTreeSet<_> = old_users.keys().copied().collect();
            if ids != input.users.iter().map(|user| user.id).collect() || ids.len() != input.users.len() {
                return Err(format!("internal mutator immutable user inventory differs: {}/{factory}", input.name));
            }
            let aliases = if factory { owner.disabled_active_user_aliases(&input.name)? } else { BTreeSet::new() };
            let mut current_users = BTreeMap::new();
            for user in &input.users {
                let before = &old_users[&user.id];
                let after = apply_user(&input.name, before, user)?;
                if &after != before { changed_users.insert(user.id); }
                current_users.insert(user.id, after);
            }
            users.insert((input.name.clone(), factory), CapturedUsers { states: current_users, active_aliases: aliases });
            let original = base.replica_runtime(&input.name, factory)?.ok_or("internal mutator scoped runtime owner absent")?;
            let times: [i64; super::owner::usage::REASONS] = input.usage.clone().try_into().map_err(|_| "internal mutator usage arity differs")?;
            let mut current = original.clone(); current.usage = times; current.override_seinfo = input.override_seinfo.clone();
            runtime_changed |= &current != original;
            runtime.insert((input.name.clone(), factory), current);
            if !factory {
                labels.insert(input.name.clone(), input.override_seinfo.clone());
                if usage.times(&input.name).is_none() { return Err("internal mutator active usage owner absent".into()); }
                for (reason, time) in times.into_iter().enumerate() { usage.notify(&input.name, reason as i32, time); }
            }
            let settings = if factory { &mut scan.settings.disabled_system_packages } else { &mut scan.settings.packages };
            let target = settings.iter_mut().find(|setting| setting.name == input.name).ok_or("internal mutator setting disappeared")?;
            target.private_flags = input.private_flags; target.category_hint = input.category;
            target.page_size_compat = input.page_flags; target.update_available = input.update_available;
            target.loading_progress = input.loading_progress; target.loading_completed_time = input.loading_completed;
            runtime_changed |= target.transient.hidden_until_installed != input.hidden_until_installed;
            target.transient.hidden_until_installed = input.hidden_until_installed;
            // InstallSource.setInstallerPackage ignores a UID-only update when
            // the installer name equals the existing immutable source name.
            if target.install_source.installer != input.installer {
                target.install_source.installer = input.installer.clone();
                target.install_source.installer_uid = input.installer_uid;
            } else if target.install_source.installer_uid != input.installer_uid {
                return Err("internal mutator installer UID changed without its name".into());
            }
            target.install_source.update_owner = input.update_owner.clone();
            let incoming_groups = input.mime_groups.as_ref().map(|groups| groups.iter().map(|(name, types)|
                (Some(name.clone()), types.iter().cloned().map(Some).collect())).collect::<Vec<_>>()).unwrap_or_default();
            let old_names: BTreeSet<_> = target.mime_groups.iter().map(|(name, _)| name.clone()).collect();
            let new_names: BTreeSet<_> = incoming_groups.iter().map(|(name, _)| name.clone()).collect();
            if old_names != new_names { return Err("internal mutator declared MIME group inventory differs".into()); }
            target.mime_groups = incoming_groups;
            if !factory { scan.installers.add(&target.install_source); }
        }
    }
    // Actual factory aliases are retained, never guessed from equal values.
    // Alias disagreement between the two incoming views rejects atomically.
    scan.capture_user_states(users)?;
    // This primitive updates the retained seInfo label owner and refreshes its
    // input identity for mutable private flags without recomputing SELinux policy.
    scan.apply_internal_mutator_seinfo(labels)?;
    scan.capture_replica_runtime(runtime)?;
    let settings_changed = scan.settings != owner.settings;
    Ok(Applied { scan, usage, settings_changed, changed_users, runtime_changed })
}
fn validate_setting(owner: &SigningScan, input: &Setting, factory: bool) -> Result<(), String> {
    if input.factory != factory { return Err("internal mutator active/factory scope changed".into()); }
    let settings = if factory { &owner.settings.disabled_system_packages } else { &owner.settings.packages };
    let original = settings.iter().find(|setting| setting.name == input.name).ok_or("internal mutator unknown setting")?;
    if input.app_id != original.app_id { return Err("internal mutator appId changed".into()); }
    if input.users.iter().any(|user| user.id < 0) { return Err("internal mutator negative user ID".into()); }
    if !input.loading_progress.is_finite() { return Err("internal mutator nonfinite loading progress".into()); }
    // PackageStateWrite exposes required-for-system-user, not privilege or UID
    // identity flags. Those require their native scan owner and cannot mutate
    // through a callback result image.
    if (input.private_flags ^ original.private_flags) & !(1 << 9) != 0 {
        return Err("internal mutator changed immutable private identity flags".into());
    }
    Ok(())
}
fn apply_user(package: &str, before: &UserState, input: &User) -> Result<UserState, String> {
    let mut state = before.clone();
    state.installed = input.installed; state.uninstall_reason = input.uninstall_reason;
    state.distraction_flags = input.distraction_flags; state.hidden = input.hidden;
    state.stopped = input.stopped; state.not_launched = input.not_launched;
    state.harmful_app_warning = input.warning.clone(); state.splash_screen_theme = input.splash.clone();
    state.min_aspect_ratio = input.min_aspect_ratio;
    if input.overrides.as_ref().is_some_and(|entries| entries.iter().any(|(component, _)| component.package != package)) {
        return Err("internal mutator override component belongs to a foreign package".into());
    }
    // Replace exact nullable/allocated-empty owners. Calling setters with an
    // invented library key to allocate an empty map would be a fake operation.
    state.runtime.restore_internal_mutation_parts(input.overlays.clone(), input.libraries.clone(), input.overrides.clone())?;
    let mut suspended = BTreeSet::new();
    if let Some(entries) = &input.suspensions {
        for entry in entries {
            let super::restrictions::SuspendingUser::Resolved(user) = entry.user else { return Err("internal mutator unresolved suspension owner".into()); };
            if !suspended.insert((user, entry.package.clone())) { return Err("internal mutator duplicate suspension owner".into()); }
        }
    }
    state.suspensions = input.suspensions.clone();
    Ok(state)
}

// Root invokes these owner-local implementation macros in the three owner
// modules. They contain the concrete operations, not callback placeholders;
// field access remains in each field's owning module.
#[macro_export]
macro_rules! install_mutator_disabled_alias_accessor {
    () => {
        impl $crate::package::scan::SigningScan {
            pub fn disabled_active_user_aliases(&self, name: &str) -> Result<std::collections::BTreeSet<i32>, String> {
                self.disabled_users.get(name).map(|owner| owner.aliases.clone())
                    .ok_or_else(|| "internal mutator disabled alias owner absent".into())
            }
        }
    };
}
#[macro_export]
macro_rules! install_mutator_seinfo_apply {
    () => {
        impl $crate::package::scan::SigningScan {
            pub fn apply_internal_mutator_seinfo(&mut self, labels: std::collections::BTreeMap<String, Option<String>>) -> Result<(), String> {
                let refreshed = inputs(self)?;
                let expected: std::collections::BTreeSet<_> = self.settings.packages.iter().map(|package| package.name.clone()).collect();
                if labels.keys().cloned().collect::<std::collections::BTreeSet<_>>() != expected {
                    return Err("internal mutator seInfo inventory differs".into());
                }
                let current = self.seinfo.as_mut().ok_or("internal mutator seInfo owner absent")?;
                for (name, value) in labels {
                    if let Some(label) = current.labels.get_mut(&name) { label.override_label = value; }
                    else if self.loaded.contains_key(&name) { return Err("internal mutator loaded seInfo owner absent".into()); }
                }
                current.inputs = refreshed;
                self.validate_seinfo()
            }
        }
    };
}
#[macro_export]
macro_rules! install_mutator_user_runtime_restore {
    () => {
        impl $crate::package::owner::user_runtime::State {
            pub fn restore_internal_mutation_parts(&mut self,
                overlays: Option<$crate::package::model::OverlayPaths>,
                libraries: Option<Vec<(String, Option<$crate::package::model::OverlayPaths>)>>,
                overrides: Option<Vec<($crate::package::owner::user_runtime::Component, $crate::package::owner::user_runtime::LabelIcon)>>,
            ) -> Result<(), String> {
                let mut names = std::collections::BTreeSet::new();
                let libraries = match libraries {
                    None => None,
                    Some(values) => {
                        let mut out = Vec::new();
                        for (name, paths) in values {
                            if !names.insert(name.clone()) { return Err("internal mutator duplicate library overlays".into()); }
                            let paths = paths.ok_or("internal mutator null library overlay entry")?;
                            out.push((name, paths));
                        }
                        Some(out)
                    }
                };
                let mut components = std::collections::BTreeSet::new();
                if let Some(values) = &overrides {
                    for (component, _) in values {
                        if !components.insert((component.package.clone(), component.class.clone())) {
                            return Err("internal mutator duplicate component overrides".into());
                        }
                    }
                }
                self.overlays = overlays;
                self.libraries = libraries;
                self.overrides = overrides;
                Ok(())
            }
        }
    };
}

#[cfg(test)]
mod aspect_ratio_tests {
    use super::*;
    #[test]
    fn mutator_replaces_aspect_ratio_and_preserves_unwritten_user_fields() {
        let before = UserState { min_aspect_ratio: 7, ce_data_inode: 123, first_install_time: 456, ..Default::default() };
        let mut user = User { id: 10, installed: true, uninstall_reason: 0, distraction_flags: 0,
            hidden: false, stopped: false, not_launched: false, warning: None, splash: None,
            min_aspect_ratio: 3, overlays: None, libraries: None, suspensions: None, overrides: None };
        let updated = apply_user("package", &before, &user).unwrap();
        assert_eq!(updated.min_aspect_ratio, 3);
        assert_eq!(updated.ce_data_inode, 123);
        assert_eq!(updated.first_install_time, 456);
        user.min_aspect_ratio = 0;
        assert_eq!(apply_user("package", &updated, &user).unwrap().min_aspect_ratio, 0);
    }
}

/// Apply only fields changed by the retained callback to the newest native
/// owner. Unrelated publications (e.g. app-data inodes) survive; overlapping
/// writes reject unless the newest value already equals the callback result.
pub fn apply_rebased(base: &Snapshot, latest: &Snapshot, record: &Record) -> Result<Applied, String> {
    if record.version != i64::try_from(base.version()).map_err(|_| "mutation version overflow")? {
        return Err("internal mutator reservation version differs".into());
    }
    if base.version() == latest.version() { return apply(base, record); }
    let mut merged = record.clone();
    for (inputs, factory) in [(&mut merged.active, false), (&mut merged.disabled, true)] {
        let before_settings = if factory { &base.owner().settings.disabled_system_packages } else { &base.owner().settings.packages };
        let current_settings = if factory { &latest.owner().settings.disabled_system_packages } else { &latest.owner().settings.packages };
        let expected = before_settings.iter().map(|setting|setting.name.as_str()).collect::<BTreeSet<_>>();
        if expected != current_settings.iter().map(|setting|setting.name.as_str()).collect() {
            return Err("internal mutator concurrent package inventory change".into());
        }
        for input in inputs {
            validate_setting(base.owner(), input, factory)?;
            let before = before_settings.iter().find(|setting|setting.name==input.name).ok_or("mutation base setting missing")?;
            let current = current_settings.iter().find(|setting|setting.name==input.name).ok_or("mutation current setting missing")?;
            if before.app_id!=current.app_id || before.code_path!=current.code_path || before.version_code!=current.version_code
                || before.signatures!=current.signatures || before.shared_user!=current.shared_user || before.shared_user_app_id!=current.shared_user_app_id
                || base.owner().loaded_packages().get(&input.name)!=latest.owner().loaded_packages().get(&input.name) {
                return Err(format!("internal mutator concurrent code identity change: {}",input.name));
            }
            macro_rules! patch { ($field:ident,$old:expr,$new:expr) => {
                input.$field = merge_field(&input.$field, &$old, &$new, stringify!($field))?;
            }; }
            patch!(private_flags,before.private_flags,current.private_flags);
            patch!(category,before.category_hint,current.category_hint);
            patch!(page_flags,before.page_size_compat,current.page_size_compat);
            patch!(update_available,before.update_available,current.update_available);
            patch!(loading_progress,before.loading_progress,current.loading_progress);
            patch!(loading_completed,before.loading_completed_time,current.loading_completed_time);
            patch!(hidden_until_installed,before.transient.hidden_until_installed,current.transient.hidden_until_installed);
            let old_runtime=base.replica_runtime(&input.name,factory)?.ok_or("mutation base runtime missing")?;
            let new_runtime=latest.replica_runtime(&input.name,factory)?.ok_or("mutation current runtime missing")?;
            patch!(override_seinfo,old_runtime.override_seinfo,new_runtime.override_seinfo);
            patch!(usage,old_runtime.usage.to_vec(),new_runtime.usage.to_vec());
            // Installer name and UID form one immutable attribution value.
            let pair=merge_field(&(input.installer.clone(),input.installer_uid),&(before.install_source.installer.clone(),before.install_source.installer_uid),&(current.install_source.installer.clone(),current.install_source.installer_uid),"installer")?;
            input.installer=pair.0;input.installer_uid=pair.1;
            patch!(update_owner,before.install_source.update_owner,current.install_source.update_owner);
            let groups=|setting:&super::settings::Package|->Result<Option<Vec<(String,Vec<String>)>>,String>{
                let values=setting.mime_groups.iter().map(|(name,types)|Ok((name.clone().ok_or("mutation nullable declared MIME group")?,types.iter().map(|value|value.clone().ok_or_else(||"mutation nullable MIME type".into())).collect::<Result<Vec<_>,String>>()?))).collect::<Result<Vec<_>,String>>()?;
                Ok(input.mime_groups.as_ref().map(|_|values))
            };
            let old_groups=groups(before)?;let new_groups=groups(current)?;
            patch!(mime_groups,old_groups,new_groups);
            let before_users=if factory{base.owner().disabled_user_states(&input.name)}else{base.owner().scanned_user_states(&input.name)}.ok_or("mutation base users missing")?;
            let current_users=if factory{latest.owner().disabled_user_states(&input.name)}else{latest.owner().scanned_user_states(&input.name)}.ok_or("mutation current users missing")?;
            if before_users.keys().ne(current_users.keys()) { return Err("internal mutator concurrent user inventory change".into()); }
            for user in &mut input.users {
                let before=before_users.get(&user.id).ok_or("mutation user absent from reservation")?;
                let current=current_users.get(&user.id).ok_or("mutation user removed")?;
                merge_user(user,before,current)?;
            }
        }
    }
    merged.version=i64::try_from(latest.version()).map_err(|_|"mutation current version overflow")?;
    apply(latest,&merged)
}
fn merge_field<T: PartialEq+Clone>(incoming:&T,before:&T,current:&T,field:&str)->Result<T,String>{
    if incoming==before { return Ok(current.clone()); }
    if current!=before && current!=incoming { return Err(format!("internal mutator concurrent field conflict: {field}")); }
    Ok(incoming.clone())
}
fn merge_user(input:&mut User,before:&UserState,current:&UserState)->Result<(),String>{
    macro_rules! patch { ($field:ident,$old:expr,$new:expr) => {
        input.$field=merge_field(&input.$field,&$old,&$new,stringify!($field))?;
    }; }
    patch!(installed,before.installed,current.installed);
    patch!(uninstall_reason,before.uninstall_reason,current.uninstall_reason);
    patch!(distraction_flags,before.distraction_flags,current.distraction_flags);
    patch!(hidden,before.hidden,current.hidden);patch!(stopped,before.stopped,current.stopped);
    patch!(not_launched,before.not_launched,current.not_launched);
    patch!(warning,before.harmful_app_warning,current.harmful_app_warning);
    patch!(splash,before.splash_screen_theme,current.splash_screen_theme);
    patch!(min_aspect_ratio,before.min_aspect_ratio,current.min_aspect_ratio);
    patch!(overlays,before.runtime.overlays().cloned(),current.runtime.overlays().cloned());
    let libraries=|state:&UserState|state.runtime.libraries().map(|values|values.iter().map(|(name,paths)|(name.clone(),Some(paths.clone()))).collect::<Vec<_>>());
    patch!(libraries,libraries(before),libraries(current));
    patch!(overrides,before.runtime.overrides().map(|values|values.to_vec()),current.runtime.overrides().map(|values|values.to_vec()));
    patch!(suspensions,before.suspensions,current.suspensions);
    Ok(())
}

#[cfg(test)]
mod interleaving_tests {
    use super::*;
    #[test]
    fn overlay_patch_preserves_concurrent_data_owner_and_rejects_overlay_conflict(){
        let before=UserState{ce_data_inode:11,..Default::default()};
        let mut current=before.clone();current.ce_data_inode=99;current.de_data_inode=101;
        let overlay=super::super::model::OverlayPaths{resource_dirs:vec!["overlay".into()],overlay_paths:vec![]};
        let mut input=User{id:0,installed:true,uninstall_reason:0,distraction_flags:0,hidden:false,stopped:false,not_launched:false,warning:None,splash:None,min_aspect_ratio:0,overlays:Some(overlay.clone()),libraries:None,suspensions:None,overrides:None};
        merge_user(&mut input,&before,&current).unwrap();
        let applied=apply_user("package",&current,&input).unwrap();
        assert_eq!(applied.ce_data_inode,99);assert_eq!(applied.de_data_inode,101);
        assert_eq!(applied.runtime.overlays(),Some(&overlay));assert_eq!(before.ce_data_inode,11);
        current.runtime.set_overlay_paths(Some(super::super::model::OverlayPaths{resource_dirs:vec!["other".into()],overlay_paths:vec![]}));
        assert!(merge_user(&mut input,&before,&current).unwrap_err().contains("overlays"));
    }
}

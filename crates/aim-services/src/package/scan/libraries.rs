//! Library candidates from accepted native code/settings/users (#808, #836).
use super::SigningScan;
use crate::package::libraries::{GraphError, Policy, ResolveError, ScanPackage, ScanResolved};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Assignments {
    registry: crate::package::libraries::Registry,
    packages: BTreeMap<String, ScanPackage>,
    policies: BTreeMap<String, (bool, bool)>,
}

impl SigningScan {
    /// Compute the complete active code graph before committing dependency
    /// state, static-library user changes and overlays at the owning stage.
    pub fn resolve_library_dependencies(
        &self,
        policy: &dyn Fn(&str, &crate::package::pkg::AndroidPackage) -> Result<Policy, ResolveError>,
    ) -> Result<ScanResolved, GraphError> {
        let available = self.library_inputs()?;
        self.libraries.resolve_scan(&available, &|package| {
            policy(&package.code.package_name, &package.code)
        })
    }

    fn library_inputs(&self) -> Result<BTreeMap<String, ScanPackage>, GraphError> {
        let error = |name: &str, input| GraphError {
            package: name.into(),
            cause: ResolveError::Incomplete(input),
        };
        if !self.capture_ready() {
            return Err(error("", "scan metadata is not finalized"));
        }
        let mut available = BTreeMap::new();
        for (name, loaded) in &self.loaded {
            let setting = self
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| error(name, "library setting owner"))?;
            if loaded.package.package_name != *name
                || self
                    .validate_collected_uid(setting, &loaded.package, true)
                    .is_err()
                || loaded.package.path.as_deref() != Some(setting.code_path.as_str())
            {
                return Err(error(name, "library code setting identity"));
            }
            let users = self
                .scanned_user_states(name)
                .ok_or_else(|| error(name, "library user owner"))?;
            available.insert(
                name.clone(),
                ScanPackage {
                    code: Arc::new(loaded.package.clone()),
                    signatures: setting.signatures.clone(),
                    users: users.clone(),
                    // Fresh native dependency candidates, computed by the resolver.
                    uses_library_files: Vec::new(),
                    uses_library_infos: Vec::new(),
                },
            );
        }
        Ok(available)
    }

    /// Commit dependency metadata and the original static-provider installed
    /// bits together only after every package resolves successfully.
    pub fn complete_library_dependencies(
        &mut self,
        policy: &dyn Fn(&str, &crate::package::pkg::AndroidPackage) -> Result<Policy, ResolveError>,
    ) -> Result<(), GraphError> {
        let policies = std::cell::RefCell::new(BTreeMap::new());
        let resolved = self.resolve_library_dependencies(&|name, package| {
            let value = policy(name, package)?;
            policies.borrow_mut().insert(name.to_owned(), (value.enforce_native_dependencies, value.sdk_library_independence));
            Ok(value)
        })?;
        for (name, package) in &resolved.packages {
            self.scanned_users
                .insert(name.clone(), package.users.clone());
            self.update_disabled_user_aliases(name, &package.users);
        }
        self.libraries = resolved.registry.clone();
        self.library_dependencies = Some(Assignments {
            registry: resolved.registry,
            packages: resolved.packages,
            policies: policies.into_inner(),
        });
        Ok(())
    }

    /// User installation/removal changes do not replace parsed code or its
    /// original compatibility policy. Resolve again from those retained inputs
    /// so installed provider bits and dependency records commit together.
    pub(in crate::package) fn complete_retained_library_dependencies(&mut self) -> Result<(), String> {
        let Some(assigned) = &self.library_dependencies else {
            if self.loaded.is_empty() { return Ok(()); }
            return Err("removal library policy owner unavailable".into());
        };
        let mut policies = BTreeMap::new();
        for (name, code) in &self.loaded {
            let previous = assigned.packages.get(name).ok_or_else(|| format!("removal library code owner unavailable: {name}"))?;
            let setting = self.settings.packages.iter().find(|setting| &setting.name == name)
                .ok_or_else(|| format!("removal library setting unavailable: {name}"))?;
            if previous.code.as_ref() != &code.package || previous.signatures != setting.signatures {
                return Err(format!("removal library code/policy identity differs: {name}"));
            }
            policies.insert(name.clone(), *assigned.policies.get(name)
                .ok_or_else(|| format!("removal original library policy unavailable: {name}"))?);
        }
        self.complete_library_dependencies(&|name, _| {
            let (native, sdk) = policies.get(name).copied()
                .ok_or(crate::package::libraries::ResolveError::Incomplete("removal original library policy"))?;
            Ok(Policy { enforce_native_dependencies: native, sdk_library_independence: sdk })
        }).map_err(|error| format!("removal library dependency completion: {error:?}"))
    }

    pub(in crate::package) fn refresh_library_user_inputs(&mut self) -> Result<(), String> {
        self.validate_library_dependencies()?;
        if let Some(assigned) = &mut self.library_dependencies {
            for (name, package) in &mut assigned.packages {
                package.users = self.scanned_users.get(name)
                    .ok_or_else(|| format!("library user owner missing: {name}"))?.clone();
            }
        }
        Ok(())
    }

    pub(in crate::package) fn validate_library_dependencies(&self) -> Result<(), String> {
        let Some(assigned) = &self.library_dependencies else {
            return Ok(());
        };
        if !self.capture_ready()
            || self.libraries != assigned.registry
            || self.loaded.keys().ne(assigned.packages.keys())
        {
            return Err("library dependency inventory differs".into());
        }
        for (name, prior) in &assigned.packages {
            let loaded = &self.loaded[name];
            let setting = self
                .settings
                .packages
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("library setting owner missing: {name}"))?;
            let users = self
                .scanned_user_states(name)
                .ok_or_else(|| format!("library user owner missing: {name}"))?;
            if &loaded.package != prior.code.as_ref()
                || setting.signatures != prior.signatures
                || loaded.package.package_name != *name
                || self
                    .validate_collected_uid(setting, &loaded.package, true)
                    .is_err()
                || loaded.package.path.as_deref() != Some(setting.code_path.as_str())
                || users.len() != prior.users.len()
                || users.iter().any(|(id, state)| {
                    prior.users.get(id).map(|p| p.installed) != Some(state.installed)
                })
            {
                return Err(format!("library dependency inputs differ: {name}"));
            }
        }
        Ok(())
    }

    pub fn library_dependencies(
        &self,
        name: &str,
    ) -> Result<Option<(&[Option<String>], &[crate::package::model::SharedLibrary])>, String> {
        self.validate_library_dependencies()?;
        self.validated_library_dependencies(name)
    }

    /// Internal batch accessor after full graph validation; immutable borrow
    /// prevents mutation between that validation and scoped record reads.
    pub(in crate::package) fn validated_library_dependencies(
        &self, name: &str,
    ) -> Result<Option<(&[Option<String>], &[crate::package::model::SharedLibrary])>, String> {
        let assigned = self
            .library_dependencies
            .as_ref()
            .ok_or("library dependencies are not resolved")?;
        Ok(assigned.packages.get(name).map(|package| {
            (
                package.uses_library_files.as_slice(),
                package.uses_library_infos.as_slice(),
            )
        }))
    }
}

#[cfg(test)]
mod install_regression {
    use super::*;
    use crate::package::{pkg::AndroidPackage,scan::LoadedPackage,settings::{Package,Settings},sign::SigningDetails,system_config::{SystemConfig,Library},owner::{seinfo,usage::Usage},scan_snapshot::Store};
    fn loaded(setting:&Package,uses:Vec<String>)->Arc<LoadedPackage>{
        let signing=SigningDetails{unknown:false,signatures:vec![vec![1]],scheme_version:2,public_keys:Some(vec![]),..SigningDetails::unknown()};
        let package=AndroidPackage{feature_flag_state:Some(vec![]),package_name:setting.name.clone(),path:Some(setting.code_path.clone()),base_apk_path:Some(format!("{}/base.apk",setting.code_path)),uid:setting.app_id,version_code:setting.version_code as i32,target_sdk_version:35,uses_libraries:uses,signing_details:signing.package_details().unwrap(),..Default::default()};
        Arc::new(LoadedPackage::new(package,signing).unwrap())
    }
    #[test]
    fn removal_reuses_original_sdk_optional_policy_and_rejects_changed_code() {
        let setting=Package{name:"optional.consumer".into(),app_id:10120,code_path:"/data/app/optional-consumer".into(),..Default::default()};
        let settings=Settings{packages:vec![setting.clone()],..Default::default()};
        let mut owner=SigningScan::new(&SystemConfig::default(),&settings,29).unwrap();
        let mut code=loaded(&setting,vec![]);let parsed=&mut Arc::make_mut(&mut code).package;
        parsed.uses_sdk_libraries=vec!["absent.sdk".into()];parsed.uses_sdk_libraries_versions_major=Some(vec![1]);parsed.uses_sdk_libraries_optional=Some(vec![true]);
        owner.loaded.insert(setting.name.clone(),code.clone());owner.scanned_users.insert(setting.name.clone(),[(0,Default::default())].into());
        owner.complete_library_dependencies(&|_,_|Ok(Policy{enforce_native_dependencies:true,sdk_library_independence:true})).unwrap();
        let mut state=owner.scanned_user_states(&setting.name).unwrap()[&0].clone();state.installed=false;owner.set_user_state(&setting.name,0,state).unwrap();
        owner.complete_retained_library_dependencies().unwrap();assert!(owner.library_dependencies(&setting.name).unwrap().unwrap().0.is_empty());
        // A missing required SDK stays rejected even when the genuine policy
        // permits optional SDK independence.
        let mut required=owner.clone();let mut changed=code;Arc::make_mut(&mut changed).package.uses_sdk_libraries_optional=Some(vec![false]);required.loaded.insert(setting.name.clone(),changed);
        assert!(required.complete_retained_library_dependencies().unwrap_err().contains("code/policy identity differs"));
        let error=required.complete_library_dependencies(&|_,_|Ok(Policy{enforce_native_dependencies:true,sdk_library_independence:true})).unwrap_err();
        assert_eq!(error.cause,ResolveError::MissingLibrary("absent.sdk".into()));
    }
    #[test]
    fn new_loaded_install_rebuilds_graph_and_runtime_before_permission_projection(){
        let mut config=SystemConfig::default();config.libraries.insert("fixture.library".into(),Library{name:"fixture.library".into(),filename:"/system/framework/fixture.jar".into(),dependencies:vec![],on_bootclasspath_since:None,on_bootclasspath_before:None,can_be_safely_ignored:false,native:false});
        let old=Package{name:"old.fixture".into(),app_id:10100,code_path:"/data/app/old.fixture-1".into(),version_code:1,domain_set_id:Some("00000000-0000-0000-0000-000000000001".into()),..Default::default()};
        let mut settings=Settings::default();settings.packages.push(old.clone());let mut initial=SigningScan::new(&config,&settings,29).unwrap();initial.loaded.insert(old.name.clone(),loaded(&old,vec![]));initial.scanned_users.insert(old.name.clone(),[(0,Default::default())].into());
        initial.assign_seinfo_at_boot(&seinfo::Policy::unread(),&mut|package|Ok(package.target_sdk_version)).unwrap();initial.complete_library_dependencies(&|_,_|Ok(Policy::pinned(true))).unwrap();let usage=Usage::new([old.name.as_str()]);initial.complete_runtime_at_boot(&usage,BTreeMap::new()).unwrap();
        initial.settings.shared_users=initial.identities.ordered_shared_users().unwrap().into_iter().map(|(name,group)|crate::package::settings::SharedUser{name:name.into(),app_id:group.app_id,flags:0,signatures:group.signatures.clone()}).collect();
        settings=initial.settings.clone();
        let store=Arc::new(Store::new(initial,usage).unwrap());let base=store.capture();
        let new=Package{name:"new.fixture".into(),app_id:10101,code_path:"/data/app/new.fixture-2".into(),version_code:2,domain_set_id:Some("00000000-0000-0000-0000-000000000002".into()),..Default::default()};settings.packages.push(new.clone());let mut candidate=SigningScan::new(&config,&settings,29).unwrap();candidate.loaded=base.owner().loaded.clone();candidate.loaded.insert(new.name.clone(),loaded(&new,vec!["fixture.library".into()]));candidate.scanned_users=base.owner().scanned_users.clone();candidate.scanned_users.insert(new.name.clone(),[(0,Default::default())].into());candidate.library_dependencies=base.owner().library_dependencies.clone();candidate.replica_runtime=base.owner().replica_runtime.clone();
        candidate.assign_seinfo_at_boot(&seinfo::Policy::unread(),&mut|package|Ok(package.target_sdk_version)).unwrap();assert_eq!(candidate.library_dependencies(&new.name).unwrap_err(),"library dependency inventory differs");
        let calls=std::cell::RefCell::new(Vec::new());candidate.complete_library_dependencies(&|name,package|{calls.borrow_mut().push((name.to_owned(),package.target_sdk_version));Ok(Policy::pinned(true))}).unwrap();assert_eq!(calls.borrow().len(),2);assert_eq!(candidate.library_dependencies(&new.name).unwrap().unwrap().0,[Some("/system/framework/fixture.jar".into())]);
        let usage=base.usage().for_install([old.name.as_str(),new.name.as_str()]);crate::package::installer::permission_prepare::complete_candidate_runtime(&mut candidate,&base,&usage).unwrap();
        let runtime=candidate.replica_runtime(&new.name,false).unwrap().unwrap();assert_eq!(runtime.library_files,[Some("/system/framework/fixture.jar".into())]);assert_eq!(runtime.usage,[0;8]);
        // Actual concurrent writer advances usage after install captured its
        // base. Atomic native installation must preserve that publication.
        let writer=store.clone();let prior=base.clone();let old_name=old.name.clone();std::thread::spawn(move||{
            let mut usage=prior.usage().clone();usage.notify(&old_name,2,998);let mut owner=prior.owner().clone();
            let mut state=owner.scanned_user_states(&old_name).unwrap()[&0].clone();state.stopped=true;state.hidden=true;
            state.runtime.set_overlay_paths(Some(crate::package::model::OverlayPaths{resource_dirs:vec!["/product/overlay/concurrent.apk".into()],overlay_paths:vec![]}));
            owner.set_user_state(&old_name,0,state).unwrap();owner.update_replica_usage(prior.usage(),&usage).unwrap();writer.publish(&prior,owner,usage).unwrap();
        }).join().unwrap();
        let latest=store.capture();
        let targets=[new.name.clone()].into();
        let mut conflict=candidate.clone();let mut state=conflict.scanned_user_states(&old.name).unwrap()[&0].clone();state.not_launched=true;
        conflict.set_user_state(&old.name,0,state).unwrap();
        assert!(!crate::package::scan_snapshot::install_context::rebase_install_users(&base,&latest,&mut conflict,&targets).unwrap());
        let mut protected=candidate.clone();let targets=[old.name.clone(),new.name.clone()].into();
        assert!(!crate::package::scan_snapshot::install_context::rebase_install_users(&base,&latest,&mut protected,&targets).unwrap());
        let mut uninstalled=latest.owner().clone();let mut state=uninstalled.scanned_user_states(&old.name).unwrap()[&0].clone();state.installed=false;
        uninstalled.set_user_state(&old.name,0,state).unwrap();uninstalled.complete_library_dependencies(&|_,_|Ok(Policy::pinned(true))).unwrap();
        crate::package::installer::permission_prepare::complete_candidate_runtime(&mut uninstalled,&latest,latest.usage()).unwrap();
        let uninstalled=Store::new(uninstalled,latest.usage().clone()).unwrap().capture();let mut rejected=candidate.clone();
        assert!(!crate::package::scan_snapshot::install_context::rebase_install_users(&base,&uninstalled,&mut rejected,&[new.name.clone()].into()).unwrap());
        // Permission preparation must use the original admission base, not a
        // later capture sampled after reserve/code/library work. A second
        // genuine publication exposes the competing-state false conflict.
        let mut advanced=latest.owner().clone();let mut state=advanced.scanned_user_states(&old.name).unwrap()[&0].clone();
        state.runtime.set_overlay_paths(Some(crate::package::model::OverlayPaths{resource_dirs:vec!["/product/overlay/latest.apk".into()],overlay_paths:vec![]}));
        advanced.set_user_state(&old.name,0,state).unwrap();let advanced=store.publish(&latest,advanced,latest.usage().clone()).unwrap();
        let targets=[new.name.clone()].into();let mut wrong_base=candidate.clone();
        assert!(crate::package::scan_snapshot::install_context::install_user_rebase_conflict(&latest,&advanced,&mut wrong_base,&targets).unwrap().unwrap().contains("competing candidate state"));
        let mut permission_check=candidate.clone();
        assert!(crate::package::scan_snapshot::install_context::install_user_rebase_conflict(&base,&advanced,&mut permission_check,&targets).unwrap().is_none());
        // The checked clone is not installed into admission: final CAS merges
        // directly from original base to its latest owner, never an interim.
        let snapshot=store.publish_install_after(&base,candidate,|_|Ok(())).unwrap();
        let merged=&snapshot.owner().scanned_user_states(&old.name).unwrap()[&0];
        assert!(merged.stopped&&merged.hidden);assert_eq!(merged.runtime.overlays().unwrap().resource_dirs,["/product/overlay/latest.apk"]);
        assert_eq!(snapshot.usage().times(&old.name).unwrap()[2],998);assert_eq!(snapshot.replica_runtime(&old.name,false).unwrap().unwrap().usage[2],998);
        assert_eq!(snapshot.usage().times(&new.name).unwrap(),&[0;8]);
        assert_eq!(snapshot.owner().loaded_packages()[&new.name].package.uid,new.app_id);assert_eq!(snapshot.owner().loaded_packages()[&new.name].package.path.as_deref(),Some(new.code_path.as_str()));
        assert!(matches!(store.publish_install_after(&base,snapshot.owner().clone(),|_|Ok(())),Err(crate::package::scan_snapshot::CommitError::Snapshot(crate::package::scan_snapshot::Error::Stale))));
        // Native uninstall must update installed-dependent library records
        // before writing restrictions, then retire code/UID without losing
        // retained user state or the other package's code/runtime owner.
        let directory=std::env::temp_dir().join(format!("aim-loaded-removal-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        struct Cleanup(std::path::PathBuf);impl Drop for Cleanup{fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
        std::fs::create_dir_all(directory.join("system/users/0")).unwrap();let _cleanup=Cleanup(directory.clone());
        let mut removal_owner=snapshot.owner().clone();
        removal_owner.settings.domain_verification.active.push(crate::package::domain_verification::Package{
            name:new.name.clone(),id:new.domain_set_id.clone().unwrap(),has_auto_verify_domains:false,signature:None,
            domains:vec![],users:vec![],uri_relative_filter_groups:vec![]});
        let removal_snapshots=Arc::new(Store::new(removal_owner,snapshot.usage().clone()).unwrap());
        let mut disk=crate::package::owner::Store::create(&directory,&[0]).unwrap();
        disk.commit_domains(&removal_snapshots.capture().owner().settings.domain_verification).unwrap();
        disk.commit_scan_settings(&removal_snapshots.capture()).unwrap();
        let target_name=new.name.clone();
        let removal=crate::package::installer::removal::NativeStore{snapshots:removal_snapshots.clone(),disk:Arc::new(std::sync::Mutex::new(disk)),publish:Arc::new(move |_,next|{
            if next.owner().settings.packages.iter().any(|package|package.name==target_name){
                assert!(next.owner().settings.domain_verification.active.iter().any(|package|package.name==target_name),"cannot publish a retained live setting with its domain owner withdrawn");
            }
            Ok(())
        }),invalidate:Arc::new(||Ok(()))};
        let before=removal_snapshots.capture();let user=crate::package::restrictions::UserState{installed:false,stopped:true,not_launched:true,..before.owner().scanned_user_states(&new.name).unwrap()[&0].clone()};
        // The removal candidate remains rooted in its actual old scan while
        // an independent usage writer wins before the disk publication gate.
        let mut pending=before.owner().clone();pending.set_user_state(&new.name,0,user.clone()).unwrap();pending.complete_retained_library_dependencies().unwrap();
        crate::package::installer::permission_prepare::complete_candidate_runtime(&mut pending,&before,before.usage()).unwrap();
        let mut usage=before.usage().clone();usage.notify(&old.name,3,1200);let mut concurrent=before.owner().clone();concurrent.update_replica_usage(before.usage(),&usage).unwrap();
        let concurrent=removal_snapshots.publish(&before,concurrent,usage).unwrap();
        let persisted=std::cell::Cell::new(false);let targets=[new.name.clone()].into();
        let cleared=removal_snapshots.publish_removal_after(&before,&targets,pending,|snapshot|{
            assert_eq!(snapshot.version(),concurrent.version()+1);
            assert_eq!(snapshot.usage().times(&old.name).unwrap()[3],1200);
            assert!(!snapshot.owner().scanned_user_states(&new.name).unwrap()[&0].installed);
            removal.disk.lock().unwrap().commit_live_install_restrictions(snapshot.owner(),0,&targets,false)?;
            persisted.set(true);Ok(())
        }).unwrap();assert!(persisted.get());
        assert_eq!(cleared.replica_runtime(&old.name,false).unwrap().unwrap().usage[3],1200);assert!(!cleared.owner().scanned_user_states(&new.name).unwrap()[&0].installed);
        assert_eq!(cleared.owner().library_dependencies(&new.name).unwrap().unwrap().0,[Some("/system/framework/fixture.jar".into())]);
        let mut no_code=cleared.owner().prepare_live_code_retirement(&new.name).unwrap();
        let mut usage=cleared.usage().clone();usage.notify(&old.name,3,1300);let mut concurrent=cleared.owner().clone();concurrent.update_replica_usage(cleared.usage(),&usage).unwrap();
        let concurrent=removal_snapshots.publish(&cleared,concurrent,usage).unwrap();
        let retired=removal_snapshots.publish_removal_after(&cleared,&targets,no_code.clone(),|_|Ok(())).unwrap();
        assert_eq!(retired.replica_runtime(&old.name,false).unwrap().unwrap().usage[3],1300);
        // A different code owner cannot pass as a usage update; reject before
        // executing any persistent callback.
        let mut foreign=concurrent.owner().clone();foreign.settings.packages.iter_mut().find(|p|p.name==old.name).unwrap().version_code+=1;
        assert!(removal_snapshots.publish_removal_after(&concurrent,&targets,foreign,|_|panic!("invalid removal must not persist")).is_err());
        no_code.settings.packages.iter_mut().find(|p|p.name==old.name).unwrap().version_code+=1;
        assert!(removal_snapshots.publish_removal_after(&retired,&targets,no_code,|_|panic!("changed runtime identity must not persist")).is_err());assert!(!retired.owner().loaded_packages().contains_key(&new.name));assert_eq!(retired.owner().scanned_user_states(&new.name).unwrap()[&0],user);
        assert!(retired.owner().settings.packages.iter().any(|p|p.name==new.name&&p.app_id==new.app_id));
        assert_eq!(retired.owner().loaded_packages()[&old.name],before.owner().loaded_packages()[&old.name]);
        let removed=removal.remove_setting(&new.name).unwrap();assert!(!removed.owner().settings.domain_verification.active.iter().any(|p|p.name==new.name));assert!(!removal.disk.lock().unwrap().state().settings.domain_verification.active.iter().any(|p|p.name==new.name));assert!(!removed.owner().settings.packages.iter().any(|p|p.name==new.name));assert!(removed.owner().identities.ids.get(new.app_id).is_none());
        assert_eq!(removed.owner().loaded_packages()[&old.name],before.owner().loaded_packages()[&old.name]);assert!(!removed.usage().names().any(|name|name==new.name));
        let receipt=crate::package::installer::pipeline::PublishedInstall{generation:snapshot.version(),
            packages:vec![crate::package::installer::pipeline::InstalledPackage{name:new.name.clone(),version_code:new.version_code,user:0}],
            verified_sessions:vec![],new_installations:Default::default()};
        // A genuine unrelated overlay + usage publication must not suppress
        // the already committed target's PACKAGE_ADDED delivery.
        let mut unrelated=snapshot.owner().clone();let mut state=unrelated.scanned_user_states(&old.name).unwrap()[&0].clone();
        state.runtime.set_overlay_paths(Some(crate::package::model::OverlayPaths{resource_dirs:vec!["/product/overlay/unrelated.apk".into()],overlay_paths:vec![]}));unrelated.set_user_state(&old.name,0,state).unwrap();
        let mut usage=snapshot.usage().clone();usage.notify(&old.name,2,999);unrelated.update_replica_usage(snapshot.usage(),&usage).unwrap();
        let later=store.publish(&snapshot,unrelated,usage).unwrap();crate::package::installer::install_events::validate_monitor_publication(&snapshot,&later,&receipt).unwrap();
        let mut replaced=later.owner().clone();replaced.settings.packages.iter_mut().find(|setting|setting.name==new.name).unwrap().code_path="/data/app/replaced-target".into();
        let original_code=replaced.loaded[&new.name].clone();let mut parsed=original_code.package.clone();parsed.path=Some("/data/app/replaced-target".into());parsed.base_apk_path=Some("/data/app/replaced-target/base.apk".into());replaced.loaded.insert(new.name.clone(),Arc::new(LoadedPackage::new(parsed,original_code.collected_signing.clone()).unwrap()));
        replaced.assign_seinfo_at_boot(&seinfo::Policy::unread(),&mut|package|Ok(package.target_sdk_version)).unwrap();replaced.complete_library_dependencies(&|_,_|Ok(Policy::pinned(true))).unwrap();crate::package::installer::permission_prepare::complete_candidate_runtime(&mut replaced,&later,later.usage()).unwrap();
        let replaced=store.publish(&later,replaced,later.usage().clone()).unwrap();assert!(crate::package::installer::install_events::validate_monitor_publication(&snapshot,&replaced,&receipt).unwrap_err().contains("identity differs"));
        let mut removed=later.owner().clone();let mut state=removed.scanned_user_states(&new.name).unwrap()[&0].clone();state.installed=false;removed.set_user_state(&new.name,0,state).unwrap();
        let policies=|_:&str,_:&crate::package::pkg::AndroidPackage|Ok(Policy::pinned(true));removed.complete_library_dependencies(&policies).unwrap();crate::package::installer::permission_prepare::complete_candidate_runtime(&mut removed,&later,later.usage()).unwrap();
        let removed=store.publish(&replaced,removed,later.usage().clone()).unwrap();assert!(crate::package::installer::install_events::validate_monitor_publication(&snapshot,&removed,&receipt).unwrap_err().contains("not installed"));
    }
}

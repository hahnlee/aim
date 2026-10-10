//! Initial Settings.writePackageRestrictions state from a captured native scan.
//! Scalar/component rules ported from android-16.0.0_r1 Settings (Apache 2.0).
use super::{Store, WriteError, attribute, element};
use crate::package::{restrictions::UserState, scan::SigningScan};
use aim_android_xml::{Element, Node, Value};

impl Store {
    /// Settings.writeDefaultAppsLPr writes a container even with no pending browser.
    pub(super) fn ensure_default_apps_owner(&self,root:&mut Element,user:u32)->Result<(),WriteError> {
        if root.children().any(|entry|entry.name=="default-apps") {return Ok(());}
        let browser=self.state.users.iter().find(|(id,_)|*id==user)
            .ok_or_else(||WriteError::before("default-apps user owner absent"))?
            .1.restrictions.default_browser.as_deref();
        let mut defaults=element("default-apps");
        if let Some(browser)=browser.filter(|value|!value.is_empty()) {
            let mut entry=element("default-browser");
            attribute(&mut entry,"packageName",Some(Value::String(browser.into())));
            defaults.content.push(Node::Element(entry));
        }
        root.content.push(Node::Element(defaults));Ok(())
    }

    /// Require the exact persisted projection of this completed scan.
    pub(crate) fn validate_committed_scan(&self, scan: &SigningScan) -> Result<(), WriteError> {
        let expected = self.persistent_scan_settings(scan)?;
        if super::canonical_persistent_settings(self.state.settings.clone()) != expected
        {
            return Err(WriteError::before(
                "committed scan/settings ownership differs",
            ));
        }
        Ok(())
    }

    /// The scan owns every package/user record; missing dependencies never use
    /// constructor defaults. Resolver/domain sections come from their owners.
    pub fn commit_initial_scan_restrictions(
        &mut self,
        scan: &SigningScan,
        user: u32,
        cross_user_suspension: bool,
        mut sections: Element,
    ) -> Result<(), WriteError> {
        self.validate_committed_scan(scan)?;
        if sections.name != "package-restrictions" || sections.children().any(|e| e.name == "pkg") {
            return Err(WriteError::before(
                "initial user side-owner sections are invalid",
            ));
        }
        self.ensure_default_apps_owner(&mut sections,user)?;
        let id = i32::try_from(user).map_err(WriteError::before)?;
        let mut packages = Vec::new();
        let mut states = Vec::new();
        for setting in &scan.settings.packages {
            let state = scan
                .scanned_user_states(&setting.name)
                .and_then(|users| users.get(&id))
                .ok_or_else(|| {
                    WriteError::before(format!(
                        "initialized scan user owner is absent: {}:{id}",
                        setting.name
                    ))
                })?;
            packages.push(Node::Element(initial_package(
                &setting.name,
                state,
                id,
                cross_user_suspension,
            )?));
            states.push((setting.name.clone(), state.clone()));
        }
        packages.append(&mut sections.content);
        sections.content = packages;
        let inventory=scan.settings.packages.iter().map(|setting|setting.name.clone()).collect();
        let result = self.commit_initial_restrictions_inventory_using(user, sections, &inventory,
            |file,bytes|std::io::Write::write_all(file,bytes));
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            // Serialization/re-read normalization must not mutate live scan
            // object state (null component sets and runtime overlays included).
            self.state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .unwrap()
                .1
                .restrictions
                .packages = states;
        }
        result
    }
}

pub(super) fn initial_package(
    name: &str,
    state: &UserState,
    user: i32,
    cross_user: bool,
) -> Result<Element, WriteError> {
    let mut pkg = element("pkg");
    attribute(&mut pkg, "name", Some(Value::String(name.into())));
    for (key, value) in [
        ("ceDataInode", state.ce_data_inode),
        ("deDataInode", state.de_data_inode),
    ] {
        if value != 0 {
            attribute(&mut pkg, key, Some(Value::Long(value)));
        }
    }
    if !state.installed {
        attribute(&mut pkg, "inst", Some(Value::Bool(false)));
    }
    for (key, value) in [
        ("stopped", state.stopped),
        ("nl", state.not_launched),
        ("hidden", state.hidden),
        ("instant-app", state.instant_app),
        ("virtual-preload", state.virtual_preload),
    ] {
        if value {
            attribute(&mut pkg, key, Some(Value::Bool(true)));
        }
    }
    for (key, value) in [
        ("distraction_flags", state.distraction_flags),
        ("enabled", state.enabled),
        ("install-reason", state.install_reason),
        ("uninstall-reason", state.uninstall_reason),
        ("min-aspect-ratio", state.min_aspect_ratio),
    ] {
        if value != 0 {
            attribute(&mut pkg, key, Some(Value::Int(value)));
        }
    }
    attribute(
        &mut pkg,
        "first-install-time",
        Some(Value::LongHex(state.first_install_time)),
    );
    for (key, value) in [
        ("enabledCaller", &state.last_disable_app_caller),
        ("harmful-app-warning", &state.harmful_app_warning),
        ("splash-screen-theme", &state.splash_screen_theme),
    ] {
        if let Some(value) = value {
            attribute(&mut pkg, key, Some(Value::String(value.clone())));
        }
    }
    let owners = state.resolved_suspensions(user, cross_user);
    if !owners.is_empty() {
        attribute(&mut pkg, "suspended", Some(Value::Bool(true)));
        for (id, owner) in owners {
            let mut entry = element("suspend-params");
            attribute(
                &mut entry,
                "suspending-package",
                Some(Value::String(owner.package.clone())),
            );
            if cross_user {
                attribute(&mut entry, "suspending-user", Some(Value::Int(id)));
            }
            if let Some(params) = &owner.params {
                attribute(
                    &mut entry,
                    "quarantined",
                    Some(Value::Bool(params.quarantined)),
                );
                if let Some(dialog) = &params.dialog {
                    entry
                        .content
                        .push(Node::Element(dialog.save("dialog-info")));
                }
                for (tag, bundle) in [
                    ("app-extras", &params.app_extras),
                    ("launcher-extras", &params.launcher_extras),
                ] {
                    if let Some(bundle) = bundle {
                        entry
                            .content
                            .push(Node::Element(bundle.save(tag).map_err(WriteError::before)?));
                    }
                }
            }
            pkg.content.push(Node::Element(entry));
        }
    }
    for (key, values) in [
        ("enabled-components", &state.enabled_components),
        ("disabled-components", &state.disabled_components),
    ] {
        if let Some(values) = values.as_ref().filter(|v| !v.is_empty()) {
            let mut group = element(key);
            for name in values {
                let mut item = element("item");
                attribute(&mut item, "name", Some(Value::String(name.clone())));
                group.content.push(Node::Element(item));
            }
            pkg.content.push(Node::Element(group));
        }
    }
    if let Some(state) = &state.archive_state {
        let mut archive = element("archive-state");
        attribute(
            &mut archive,
            "installer-title",
            Some(Value::String(state.installer_title.clone())),
        );
        attribute(
            &mut archive,
            "archive-time",
            Some(Value::LongHex(state.archive_time)),
        );
        for activity in &state.activities {
            let mut entry = element("archive-activity-info");
            attribute(
                &mut entry,
                "activity-title",
                Some(Value::String(activity.title.clone())),
            );
            attribute(
                &mut entry,
                "original-component-name",
                Some(Value::String(activity.original_component_name.clone())),
            );
            for (tag, path) in [
                ("icon-path", &activity.icon_path),
                ("monochrome-icon-path", &activity.monochrome_icon_path),
            ] {
                if let Some(path) = path {
                    if !path.starts_with('/') {
                        return Err(WriteError::before(
                            "archive icon path requires guest absolute-path ownership",
                        ));
                    }
                    attribute(&mut entry, tag, Some(Value::String(path.clone())));
                }
            }
            archive.content.push(Node::Element(entry));
        }
        pkg.content.push(Node::Element(archive));
    }
    Ok(pkg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::restrictions::{Suspension, SuspendingUser};

    #[test]
    fn initialized_suspension_policy_preserves_nullable_owners_and_child_order() {
        let state = UserState {
            suspensions: Some(vec![Suspension {
                package: "owner".into(),
                user: SuspendingUser::Resolved(12),
                params: None,
            }]),
            enabled_components: Some(vec!["p.Main".into()]),
            disabled_components: Some(vec!["p.Other".into()]),
            ..Default::default()
        };
        for cross_user in [false, true] {
            let pkg = initial_package("p", &state, 10, cross_user).unwrap();
            let children: Vec<_> = pkg.content.iter().filter_map(|n| match n {
                Node::Element(e) => Some(e),
                _ => None,
            }).collect();
            assert_eq!(children.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                ["suspend-params", "enabled-components", "disabled-components"]);
            assert_eq!(children[0].int("suspending-user").unwrap(), cross_user.then_some(12));
            assert!(children[0].content.is_empty());
            assert_eq!(pkg.bool("suspended").unwrap(), Some(true));
        }
    }
}

#[cfg(test)]
mod default_apps_owner_tests {
    use super::*;
    use super::super::tests::Data;
    use crate::package::{scan::{SigningScan,CapturedUsers},restrictions::Restrictions};
    use std::{collections::BTreeMap,fs};

    fn restored(browser:Option<&str>) -> (Data,Store,SigningScan,std::path::PathBuf) {
        let data=Data::new();let path=data.settings();
        fs::write(data.0.join("system/packages.xml"),b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' domainSetId='00000000-0000-0000-0000-000000000001'/></packages>").unwrap();
        let mut root=element("package-restrictions");let mut package=element("pkg");
        attribute(&mut package,"name",Some(Value::String("example.app".into())));attribute(&mut package,"enabled",Some(Value::Int(2)));root.content.push(Node::Element(package));
        if let Some(browser)=browser {
            let mut defaults=element("default-apps");let mut selected=element("default-browser");
            attribute(&mut selected,"packageName",Some(Value::String(browser.into())));defaults.content.push(Node::Element(selected));root.content.push(Node::Element(defaults));
        }
        let mut extension=element("retained-extension");attribute(&mut extension,"owner",Some(Value::String("keep".into())));root.content.push(Node::Element(extension));
        fs::write(&path,aim_android_xml::abx::write(&root).unwrap()).unwrap();
        let mut store=Store::open(&data.0,&[0]).unwrap().unwrap();
        let mut scan=SigningScan::new(&Default::default(),&store.state().settings,36).unwrap();
        let states=store.state().users[0].1.restrictions.packages.iter().map(|(name,state)|((name.clone(),false),CapturedUsers{states:BTreeMap::from([(0,state.clone())]),active_aliases:Default::default()})).collect();
        scan.capture_user_states(states).unwrap();
        let snapshot=crate::package::scan_snapshot::Store::new(scan.clone(),super::super::usage::Usage::new(["example.app"])).unwrap().capture();
        store.commit_scan_settings(&snapshot).unwrap();
        (data,store,scan,path)
    }
    fn fresh_initial() -> (Data,Store,SigningScan,std::path::PathBuf) {
        let data=Data::new();let path=data.settings();fs::remove_file(data.0.join("system/packages.xml")).unwrap();
        let mut settings=crate::package::settings::Settings::parse(&aim_android_xml::read(b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' domainSetId='00000000-0000-0000-0000-000000000001'/></packages>").unwrap()).unwrap();
        let current=crate::package::settings::Version{sdk_version:36,database_version:3,..Default::default()};
        let(mut store,_)=super::super::recovery::Plan::inspect(&data.0).unwrap().recover_boot(&[0],&mut settings,&current,|_,_|panic!()).unwrap();
        let mut scan=SigningScan::new(&Default::default(),&settings,36).unwrap();
        scan.capture_user_states(BTreeMap::from([(("example.app".into(),false),CapturedUsers{states:BTreeMap::from([(0,crate::package::restrictions::UserState{enabled:2,..Default::default()})]),active_aliases:Default::default()})])).unwrap();
        let snapshot=crate::package::scan_snapshot::Store::new(scan.clone(),super::super::usage::Usage::new(["example.app"])).unwrap().capture();
        store.commit_scan_settings(&snapshot).unwrap();store.claim_unread_restrictions(0).unwrap();
        (data,store,scan,path)
    }
    fn read(path:&std::path::Path)->Element {aim_android_xml::read(&fs::read(path).unwrap()).unwrap()}
    fn check(path:&std::path::Path,browser:Option<&str>) {
        let root=read(path);assert_eq!(root.children().filter(|n|n.name=="default-apps").count(),1);
        let state=Restrictions::parse(&root).unwrap();assert_eq!(state.default_browser.as_deref(),browser);
        assert_eq!(state.packages.iter().find(|(n,_)|n=="example.app").unwrap().1.enabled,2);
    }
    #[test]
    fn initial_actual_store_write_retains_pending_browser_and_empty_container() {
        for browser in [None,Some("actual.browser")] {
            let (_data,mut store,scan,path)=fresh_initial();
            let mut sections=element("package-restrictions");
            if let Some(browser)=browser {
                let mut defaults=element("default-apps");let mut selected=element("default-browser");
                attribute(&mut selected,"packageName",Some(Value::String(browser.into())));defaults.content.push(Node::Element(selected));sections.content.push(Node::Element(defaults));
            }
            store.commit_initial_scan_restrictions(&scan,0,false,sections).unwrap();
            check(&path,browser);
        }
    }
    #[test]
    fn restored_actual_store_repairs_missing_container_and_retains_existing_browser() {
        for browser in [None,Some("actual.restored.browser")] {
            let (_data,mut store,scan,path)=restored(browser);
            let extension=read(&path).children().find(|n|n.name=="retained-extension").unwrap().clone();
            store.commit_updated_scan_restrictions(&scan,0,false).unwrap();check(&path,browser);
            assert!(read(&path).children().any(|n|n==&extension));
        }
    }
}

//! A user's package state, `/data/system/users/<user>/package-restrictions.xml`:
//! per package whether it is installed, enabled, stopped or hidden for the
//! user, its data directories' inodes, install reason and first install
//! time, and the components enabled or disabled at run time; the user's
//! default browser and the packages they may not uninstall. As
//! `Settings.readPackageRestrictionsLPr` reads it
//! (`writePackageRestrictions` writes it).
//!
//! Not assembled here yet (#706): preferred and persistent preferred
//! activities and cross-profile intent filters.

pub mod dialog;
pub mod persistable;

use aim_android_xml::Element;

use super::{children, string};

/// `PackageManager.COMPONENT_ENABLED_STATE_DEFAULT`.
pub const COMPONENT_ENABLED_STATE_DEFAULT: i32 = 0;

/// The pinned Settings DEX removes cross-user suspension reads and writes.
/// `pinned_settings_suspension_policy` checks both compiled methods.
pub const PINNED_CROSS_USER_SUSPENSIONS: bool = false;


#[derive(Clone, Debug, Default, PartialEq)]
pub struct Restrictions {
    /// By package name.
    pub packages: Vec<(String, UserState)>,
    /// Inputs handed separately to DomainVerificationManager by Settings.
    pub legacy_domain_states: Vec<(String, i32)>,
    pub default_browser: Option<String>,
    pub block_uninstall: Vec<String>,
}

/// `PackageUserStateImpl`: one package's state for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct UserState {
    /// Runtime fields initialized by the original user-state constructor.
    pub runtime: super::owner::user_runtime::State,
    pub ce_data_inode: i64,
    pub de_data_inode: i64,
    pub installed: bool,
    pub stopped: bool,
    pub not_launched: bool,
    pub hidden: bool,
    pub distraction_flags: i32,
    /// Suspension owners in file order. User IDs remain unresolved until
    /// the reader has the image's cross-user suspension policy.
    pub suspensions: Option<Vec<Suspension>>,
    pub instant_app: bool,
    pub virtual_preload: bool,
    /// `COMPONENT_ENABLED_STATE_*`.
    pub enabled: i32,
    pub last_disable_app_caller: Option<String>,
    /// None for the default state; initialized Settings owners may be empty.
    pub enabled_components: Option<Vec<String>>,
    pub disabled_components: Option<Vec<String>>,
    /// `PackageManager.INSTALL_REASON_*`, `UNINSTALL_REASON_*`.
    pub install_reason: i32,
    pub uninstall_reason: i32,
    pub harmful_app_warning: Option<String>,
    pub splash_screen_theme: Option<String>,
    /// In ms; 0 when the file has none (the settings' legacy `it` then
    /// applies).
    pub first_install_time: i64,
    /// `PackageManager.USER_MIN_ASPECT_RATIO_*`.
    pub min_aspect_ratio: i32,
    pub archive_state: Option<ArchiveState>,
}

impl Default for UserState {
    /// The original default user state, before Settings initializes its sets.
    fn default() -> Self {
        UserState {
            runtime: Default::default(),
            ce_data_inode: 0,
            de_data_inode: 0,
            installed: true,
            stopped: false,
            not_launched: false,
            hidden: false,
            distraction_flags: 0,
            suspensions: None,
            instant_app: false,
            virtual_preload: false,
            enabled: COMPONENT_ENABLED_STATE_DEFAULT,
            last_disable_app_caller: None,
            enabled_components: None,
            disabled_components: None,
            install_reason: 0,
            uninstall_reason: 0,
            harmful_app_warning: None,
            splash_screen_theme: None,
            first_install_time: 0,
            min_aspect_ratio: 0,
            archive_state: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suspension {
    pub package: String,
    pub user: SuspendingUser,
    pub params: Option<SuspendParams>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SuspendParams {
    pub dialog: Option<dialog::DialogInfo>,
    pub quarantined: bool,
    pub app_extras: Option<persistable::Bundle>,
    pub launcher_extras: Option<persistable::Bundle>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuspendingUser {
    /// Runtime UserPackage key; never reinterpret it with XML read policy.
    Resolved(i32),
    /// Legacy `suspended`: always the package's user, including platform suspensions.
    Current,
    /// `suspend-params`: interpret with `crossUserSuspensionEnabledRo`.
    Persisted(Option<i32>),
}

impl Suspension {
    fn read(cursor: &mut persistable::Cursor<'_>, package: String) -> Result<Self, String> {
        use persistable::Event;
        let Some(Event::Start(e, depth)) = cursor.event() else {
            return Err("expected suspension tag".into());
        };
        let user = SuspendingUser::Persisted(e.int("suspending-user").ok().flatten());
        let mut params = SuspendParams {
            quarantined: e.bool("quarantined").ok().flatten().unwrap_or(false),
            ..Default::default()
        };
        while let Some(event) = cursor.next() {
            let parameter = match event {
                Event::End(_, d) if d <= depth => break,
                Event::Start(e, _) => e,
                _ => continue,
            };
            match parameter.name.as_str() {
                "dialog-info" => params.dialog = Some(dialog::DialogInfo::restore(parameter)),
                "app-extras" | "launcher-extras" => {
                    let bundle = match persistable::Bundle::read(cursor) {
                        Ok(bundle) => bundle,
                        // SuspendParams catches XmlPullParserException, preserving
                        // fields read before the failed bundle; runtime errors escape.
                        Err(persistable::Error::Xml(_)) => break,
                        Err(error) => return Err(error.to_string()),
                    };
                    if parameter.name == "app-extras" {
                        params.app_extras = Some(bundle);
                    } else {
                        params.launcher_extras = Some(bundle);
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            package,
            user,
            params: Some(params),
        })
    }
}

impl UserState {
    /// Settings.setUserState calls the ArraySet setters even for null inputs,
    /// creating nonnull empty component owners in the initialized state.
    pub fn initialized() -> Self {
        Self {
            enabled_components: Some(Vec::new()),
            disabled_components: Some(Vec::new()),
            ..Self::default()
        }
    }
    /// `Settings.readSuspensionParamsLPr` and ArrayMap.put: resolve user
    /// ownership with the image policy, then let the last duplicate win.
    pub fn resolved_suspensions(&self, user: i32, cross_user: bool) -> Vec<(i32, &Suspension)> {
        let mut resolved: Vec<(i32, &Suspension)> = Vec::new();
        for suspension in self.suspensions.iter().flatten() {
            let owner = match suspension.user {
                SuspendingUser::Resolved(id) => id,
                SuspendingUser::Current => user,
                SuspendingUser::Persisted(_) if !cross_user => user,
                SuspendingUser::Persisted(Some(id)) if id != -10000 => id,
                SuspendingUser::Persisted(_) => match suspension.package.as_str() {
                    "root" | "com.android.shell" | "android" => 0,
                    _ => user,
                },
            };
            if let Some(entry) = resolved
                .iter_mut()
                .find(|(id, s)| *id == owner && s.package == suspension.package)
            {
                *entry = (owner, suspension);
            } else {
                resolved.push((owner, suspension));
            }
        }
        resolved.sort_by_key(|(id, s)| {
            id.wrapping_mul(31).wrapping_add(
                s.package
                    .encode_utf16()
                    .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c))),
            )
        });
        resolved
    }

    fn runtime_suspensions(&self, user: i32, cross_user: bool) -> Option<Vec<Suspension>> {
        self.suspensions.as_ref().map(|_| {
            self.resolved_suspensions(user, cross_user)
                .into_iter()
                .map(|(id, value)| Suspension {
                    package: value.package.clone(),
                    user: SuspendingUser::Resolved(id),
                    params: value.params.clone(),
                })
                .collect()
        })
    }
    /// Store a runtime UserPackage key, including an explicit null value.
    pub fn put_suspension(
        &mut self,
        user: i32,
        cross_user: bool,
        suspender: i32,
        package: String,
        params: Option<SuspendParams>,
    ) {
        let mut entries = self
            .runtime_suspensions(user, cross_user)
            .unwrap_or_default();
        if let Some(value) = entries.iter_mut().find(|value| {
            value.user == SuspendingUser::Resolved(suspender) && value.package == package
        }) {
            value.params = params;
        } else {
            entries.push(Suspension {
                package,
                user: SuspendingUser::Resolved(suspender),
                params,
            });
        }
        self.suspensions = Some(entries);
    }
    /// Original removal leaves an allocated empty map and preserves null maps.
    pub fn remove_suspension(
        &mut self,
        user: i32,
        cross_user: bool,
        suspender: i32,
        package: &str,
    ) {
        let Some(mut entries) = self.runtime_suspensions(user, cross_user) else {
            return;
        };
        entries.retain(|value| {
            value.user != SuspendingUser::Resolved(suspender) || value.package != package
        });
        self.suspensions = Some(entries);
    }

    pub fn is_quarantined(&self, user: i32, cross_user: bool) -> Result<bool, NullSuspendParams> {
        for (_, suspension) in self.resolved_suspensions(user, cross_user) {
            if suspension
                .params
                .as_ref()
                .ok_or(NullSuspendParams)?
                .quarantined
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NullSuspendParams;

/// `ArchiveState`: what the launcher shows of an archived package.
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveState {
    pub installer_title: String,
    pub archive_time: i64,
    pub activities: Vec<ArchiveActivity>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveActivity {
    pub title: String,
    pub original_component_name: String,
    pub icon_path: Option<String>,
    pub monochrome_icon_path: Option<String>,
}

impl Restrictions {
    /// The state in the document whose root is `root`. A package the
    /// settings do not know is the caller's to drop, as the original drops
    /// it.
    pub fn parse(root: &Element) -> Result<Restrictions, String> {
        let mut r = Restrictions::default();
        for e in root.children() {
            match e.name.as_str() {
                "pkg" => {
                    let Some(name) = string(e, "name") else {
                        continue;
                    };
                    // The legacy attribute; a later list replaces it.
                    if e.bool("blockUninstall")?.unwrap_or(false)
                        && !r.block_uninstall.contains(&name)
                    {
                        r.block_uninstall.push(name.clone());
                    }
                    let state = user_state(e)?;
                    let legacy = e.int("domainVerificationStatus")?.unwrap_or(0);
                    r.legacy_domain_states.push((name.clone(), legacy));
                    r.packages.push((name, state));
                }
                "default-apps" => {
                    for browser in children(e, "default-browser") {
                        r.default_browser = string(browser, "packageName");
                    }
                }
                "block-uninstall-packages" => {
                    r.block_uninstall = children(e, "block-uninstall")
                        .filter_map(|b| string(b, "packageName"))
                        .collect();
                }
                _ => {}
            }
        }
        Ok(r)
    }
}

fn user_state(e: &Element) -> Result<UserState, String> {
    let bool = |name, default| e.bool(name).map(|v| v.unwrap_or(default));
    let suspended = bool("suspended", false)?;
    let mut s = UserState {
        ce_data_inode: e.long("ceDataInode")?.unwrap_or(0),
        de_data_inode: e.long("deDataInode")?.unwrap_or(0),
        installed: bool("inst", true)?,
        stopped: bool("stopped", false)?,
        not_launched: bool("nl", false)?,
        // `blocked` is `hidden`'s old name.
        hidden: bool("hidden", false)? || bool("blocked", false)?,
        distraction_flags: e.int("distraction_flags")?.unwrap_or(0),
        instant_app: bool("instant-app", false)?,
        virtual_preload: bool("virtual-preload", false)?,
        enabled: e.int("enabled")?.unwrap_or(COMPONENT_ENABLED_STATE_DEFAULT),
        last_disable_app_caller: string(e, "enabledCaller"),
        install_reason: e.int("install-reason")?.unwrap_or(0),
        uninstall_reason: e.int("uninstall-reason")?.unwrap_or(0),
        harmful_app_warning: string(e, "harmful-app-warning"),
        splash_screen_theme: string(e, "splash-screen-theme"),
        first_install_time: e.long_hex("first-install-time")?.unwrap_or(0),
        min_aspect_ratio: e.int("min-aspect-ratio")?.unwrap_or(0),
        ..UserState::initialized()
    };
    let mut legacy_dialog = None;
    let mut legacy_app_extras = None;
    let mut legacy_launcher_extras = None;
    let mut cursor = persistable::Cursor::new(e);
    while let Some(event) = cursor.next() {
        let persistable::Event::Start(child, _) = event else {
            continue;
        };
        match child.name.as_str() {
            "enabled-components" => {
                s.enabled_components = Some(components(child));
                cursor.skip();
            }
            "disabled-components" => {
                s.disabled_components = Some(components(child));
                cursor.skip();
            }
            "suspend-params" => {
                if let Some(by) = string(child, "suspending-package") {
                    s.suspensions
                        .get_or_insert_with(Vec::new)
                        .push(Suspension::read(&mut cursor, by)?);
                }
            }
            "suspended-dialog-info" => legacy_dialog = Some(dialog::DialogInfo::restore(child)),
            "suspended-app-extras" => {
                legacy_app_extras =
                    Some(persistable::Bundle::read(&mut cursor).map_err(|e| e.to_string())?)
            }
            "suspended-launcher-extras" => {
                legacy_launcher_extras =
                    Some(persistable::Bundle::read(&mut cursor).map_err(|e| e.to_string())?)
            }
            "archive-state" => {
                s.archive_state = archive_state(child)?;
                cursor.skip();
            }
            _ => {}
        }
    }
    if suspended && s.suspensions.as_ref().is_none_or(Vec::is_empty) {
        let by = string(e, "suspending-package").unwrap_or_else(|| "android".into());
        if legacy_dialog.is_none() {
            legacy_dialog = string(e, "suspend_dialog_message")
                .filter(|m| !m.is_empty())
                .map(|message| dialog::DialogInfo {
                    message: Some(message),
                    ..dialog::DialogInfo::default()
                });
        }
        s.suspensions.get_or_insert_with(Vec::new).push(Suspension {
            package: by,
            user: SuspendingUser::Current,
            params: Some(SuspendParams {
                dialog: legacy_dialog,
                quarantined: false,
                app_extras: legacy_app_extras,
                launcher_extras: legacy_launcher_extras,
            }),
        });
    }
    Ok(s)
}

// Settings.readComponentsLPr, Android 16.0.0_r1.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
fn components(e: &Element) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cursor = persistable::Cursor::new(e);
    while let Some(event) = cursor.next() {
        if let persistable::Event::Start(item, _) = event
            && item.name == "item"
            && let Some(name) = string(item, "name")
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    // ArraySet inserts after equal hashes, preserving collision insertion order.
    out.sort_by_key(|s| {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
    });
    out
}

/// `parseArchiveState`: `None` for a state without an installer title or
/// activities, which the original drops.
fn archive_state(e: &Element) -> Result<Option<ArchiveState>, String> {
    let mut activities = Vec::new();
    let mut cursor = persistable::Cursor::new(e);
    // Settings.parseArchiveActivityInfos visits descendant start tags.
    while let Some(event) = cursor.next() {
        if let persistable::Event::Start(a, _) = event
            && a.name == "archive-activity-info"
            && let Some(title) = string(a, "activity-title")
            && let Some(component) = string(a, "original-component-name")
            && let Some((package, class)) = component.split_once('/')
            && !class.is_empty()
            && let Some(icon_path) = string(a, "icon-path")
        {
            // ComponentName.unflattenFromString expands a relative class name.
            let class = if class.starts_with('.') {
                format!("{package}{class}")
            } else {
                class.to_owned()
            };
            activities.push(ArchiveActivity {
                title,
                original_component_name: format!("{package}/{class}"),
                icon_path: Some(icon_path),
                monochrome_icon_path: string(a, "monochrome-icon-path"),
            });
        }
    }
    // The default-valued TypedXmlPullParser getter returns zero on conversion errors.
    let archive_time = e.long_hex("archive-time").ok().flatten().unwrap_or(0);
    let Some(installer_title) = string(e, "installer-title").filter(|_| !activities.is_empty())
    else {
        return Ok(None);
    };
    // ArchiveState validates its @CurrentTimeMillisLong constructor argument.
    if archive_time < 0 {
        return Err("ArchiveState: negative archive time".into());
    }
    Ok(Some(ArchiveState {
        installer_title,
        archive_time,
        activities,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(xml: &[u8]) -> UserState {
        user_state(&aim_android_xml::read(xml).unwrap()).unwrap()
    }

    #[test]
    fn legacy_domain_migration_inputs_do_not_belong_to_user_state() {
        let read = |xml: &[u8]| Restrictions::parse(&aim_android_xml::read(xml).unwrap()).unwrap();
        let active = read(br#"<package-restrictions><pkg name='p' domainVerificationStatus='2'/></package-restrictions>"#);
        let factory = read(br#"<package-restrictions><pkg name='p' domainVerificationStatus='3'/></package-restrictions>"#);
        assert_eq!(active.packages, factory.packages);
        assert_eq!(active.legacy_domain_states, [("p".into(), 2)]);
        assert_eq!(factory.legacy_domain_states, [("p".into(), 3)]);
        let default = read(br#"<package-restrictions><pkg name='p'/></package-restrictions>"#);
        assert_eq!(default.legacy_domain_states, [("p".into(), 0)]);
        assert!(Restrictions::parse(&aim_android_xml::read(br#"<package-restrictions><pkg name='p' domainVerificationStatus='bad'/></package-restrictions>"#).unwrap()).is_err());
    }

    #[test]
    fn suspension_parameters_survive_read_and_contextual_duplicate_resolution() {
        let s = state(
            br#"<pkg>
            <suspend-params suspending-package='android' quarantined='true'>
                <dialog-info title='first'/>
                <app-extras><string name='message'>kept</string></app-extras>
            </suspend-params>
            <suspend-params suspending-package='android' suspending-user='10' quarantined='false'>
                <dialog-info title='last'/>
                <launcher-extras><int name='count' value='3'/></launcher-extras>
            </suspend-params>
            <suspend-params><dialog-info title='no owner'/></suspend-params>
        </pkg>"#,
        );
        assert_eq!(s.suspensions.as_ref().unwrap().len(), 2);
        assert!(
            s.suspensions.as_ref().unwrap()[0]
                .params
                .as_ref()
                .unwrap()
                .app_extras
                .is_some()
        );
        assert!(
            s.suspensions.as_ref().unwrap()[1]
                .params
                .as_ref()
                .unwrap()
                .launcher_extras
                .is_some()
        );
        assert_eq!(
            s.resolved_suspensions(10, true)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [0, 10]
        );
        assert!(s.is_quarantined(10, true).unwrap());
        let resolved = s.resolved_suspensions(10, false);
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0]
                .1
                .params
                .as_ref()
                .unwrap()
                .dialog
                .as_ref()
                .unwrap()
                .title
                .as_deref(),
            Some("last")
        );
        assert!(!s.is_quarantined(10, false).unwrap());
    }

    #[test]
    fn legacy_suspension_uses_current_user_and_explicit_dialog_precedes_message() {
        let s = state(
            br#"<pkg suspended='true' suspend_dialog_message='fallback'>
            <suspended-dialog-info title='legacy'/>
            <suspended-app-extras><string name='old'>kept</string></suspended-app-extras>
        </pkg>"#,
        );
        let resolved = s.resolved_suspensions(10, true);
        assert_eq!(resolved[0].0, 10);
        let suspension = resolved[0].1;
        assert_eq!(suspension.package, "android");
        assert_eq!(
            suspension
                .params
                .as_ref()
                .unwrap()
                .dialog
                .as_ref()
                .unwrap()
                .title
                .as_deref(),
            Some("legacy")
        );
        assert_eq!(
            suspension
                .params
                .as_ref()
                .unwrap()
                .dialog
                .as_ref()
                .unwrap()
                .message,
            None
        );
        assert!(suspension.params.as_ref().unwrap().app_extras.is_some());
        assert!(!s.is_quarantined(10, true).unwrap());
        let fallback = state(b"<pkg suspended='true' suspend_dialog_message='fallback'/>");
        assert_eq!(
            fallback.suspensions.as_ref().unwrap()[0]
                .params
                .as_ref()
                .unwrap()
                .dialog
                .as_ref()
                .unwrap()
                .message
                .as_deref(),
            Some("fallback")
        );
        assert!(
            state(b"<pkg suspended='false' suspend_dialog_message='ignored'/>")
                .suspensions
                .is_none()
        );
    }

    #[test]
    fn runtime_put_null_and_removal_preserve_map_allocation() {
        let mut state = UserState::default();
        state.remove_suspension(10, false, 0, "android");
        assert!(state.suspensions.is_none());
        state.put_suspension(10, false, 0, "android".into(), None);
        assert_eq!(state.is_quarantined(10, true), Err(NullSuspendParams));
        assert_eq!(state.resolved_suspensions(10, false)[0].0, 0);
        let captured = state.clone();
        state.put_suspension(
            10,
            false,
            0,
            "android".into(),
            Some(SuspendParams::default()),
        );
        assert_eq!(state.suspensions.as_ref().unwrap().len(), 1);
        assert_eq!(state.is_quarantined(10, false), Ok(false));
        assert_eq!(captured.is_quarantined(10, false), Err(NullSuspendParams));
        state.remove_suspension(10, true, 0, "android");
        assert_eq!(state.suspensions, Some(vec![]));
    }

    #[test]
    fn null_params_keep_runtime_keys_and_quarantine_short_circuit_order() {
        let suspension = |package: &str, params| Suspension {
            package: package.into(),
            user: SuspendingUser::Resolved(0),
            params,
        };
        let mut state = UserState {
            suspensions: Some(vec![
                suspension(
                    "B",
                    Some(SuspendParams {
                        quarantined: true,
                        ..Default::default()
                    }),
                ),
                suspension("android", None),
            ]),
            ..Default::default()
        };
        assert_eq!(
            state
                .resolved_suspensions(10, false)
                .iter()
                .map(|(id, s)| (*id, s.package.as_str()))
                .collect::<Vec<_>>(),
            [(0, "android"), (0, "B")]
        );
        assert_eq!(state.is_quarantined(10, false), Err(NullSuspendParams));
        state.suspensions = Some(vec![
            suspension("B", None),
            suspension(
                "android",
                Some(SuspendParams {
                    quarantined: true,
                    ..Default::default()
                }),
            ),
        ]);
        assert_eq!(state.is_quarantined(10, false), Ok(true));
    }

    #[test]
    fn malformed_or_sentinel_users_follow_original_default_policy() {
        let s = state(
            br#"<pkg>
            <suspend-params suspending-package='android' suspending-user='bad'/>
            <suspend-params suspending-package='ordinary' suspending-user='-10000'/>
            <suspend-params suspending-package='root'/>
            <suspend-params suspending-package='com.android.shell'/>
        </pkg>"#,
        );
        assert_eq!(
            s.resolved_suspensions(12, true)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [0, 0, 12, 0]
        );
    }
}

#[cfg(test)]
mod component_tests {
    use super::*;
    #[test]
    fn descendant_items_follow_signed_java_hash_and_collision_order() {
        let root = aim_android_xml::read(b"<package-restrictions><pkg name='fixture'><enabled-components><item name='BB'/><unknown><item name='Aa'/><item name='B'><item name='zzzzzz'/></item></unknown><item name='BB'/><item/><item name=''/></enabled-components></pkg></package-restrictions>").unwrap();
        let state = Restrictions::parse(&root).unwrap();
        assert_eq!(
            state.packages[0].1.enabled_components.as_deref(),
            Some(["zzzzzz", "", "B", "BB", "Aa"].map(String::from).as_slice())
        );
    }
}

#[cfg(test)]
mod pinned_policy_tests {
    #[test]
    #[ignore = "requires pinned original image; run explicitly"]
    fn pinned_settings_suspension_policy() {
        use aim_android_image::dex::{Dex, instruction_units, units};
        let jar = aim_apps::apk::Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
        let mut found = false;
        for index in 1.. {
            let name = if index == 1 { "classes.dex".into() } else { format!("classes{index}.dex") };
            let Some(bytes) = jar.file_if_present(&name).unwrap() else { break };
            let dex = Dex::parse(&bytes).unwrap();
            let Some(class) = dex.class("Lcom/android/server/pm/Settings;") else { continue };
            found = true;
            let reader = dex.methods_named(class, "readSuspensionParamsLPr").unwrap();
            assert_eq!(reader.len(), 1);
            let code = units(&bytes, &reader[0]).unwrap();
            let mut opcodes = Vec::new();
            let mut at = 0;
            while at < code.len() {
                opcodes.push(code[at] & 0xff);
                at += instruction_units(&code, at).unwrap();
            }
            // This compiled reader forwards the current user directly to
            // UserPackage.of; no int attribute read or flag branch survives.
            assert_eq!(opcodes, [0x1b,0x12,0x72,0x0c,0x39,0x1a,0x1a,0x71,0x11,0x71,0x0c,0x71,0x0c,0x71,0x0c,0x11]);
            let header = reader[0].insns_off - 16;
            let registers = u16::from_le_bytes(bytes[header..header+2].try_into().unwrap());
            let inputs = u16::from_le_bytes(bytes[header+2..header+4].try_into().unwrap());
            assert_eq!((registers, inputs), (4, 2));
            assert_eq!((code[18], code[20]), (0x2071, 0x0002));
            assert_eq!(dex.method(u32::from(code[19])).unwrap(), ("Landroid/content/pm/UserPackage;".into(), "of".into(), "(ILjava/lang/String;)Landroid/content/pm/UserPackage;".into()));
            let writers = dex.methods_named(class, "writePackageRestrictions").unwrap();
            assert!(!writers.is_empty());
            let mut suspension = false;
            for writer in writers {
                let code = units(&bytes, &writer).unwrap();
                let mut at = 0;
                while at < code.len() {
                    let op = code[at] & 0xff;
                    if op == 0x1a || op == 0x1b {
                        let mut id = u32::from(code[at+1]);
                        if op == 0x1b { id |= u32::from(code[at+2]) << 16; }
                        let value = dex.string(id).unwrap();
                        assert_ne!(value, "suspending-user");
                        suspension |= value == "suspending-package";
                    }
                    at += instruction_units(&code, at).unwrap();
                }
            }
            assert!(suspension);
        }
        assert!(found, "original Settings was not inspected");
        assert!(!super::PINNED_CROSS_USER_SUSPENSIONS);
    }
}

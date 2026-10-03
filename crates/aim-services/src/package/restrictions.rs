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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Restrictions {
    /// By package name.
    pub packages: Vec<(String, UserState)>,
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
    pub suspensions: Vec<Suspension>,
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
    /// The legacy `domainVerificationStatus`, which the original hands to
    /// domain verification.
    pub domain_verification_status: i32,
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
            suspensions: Vec::new(),
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
            domain_verification_status: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suspension {
    pub package: String,
    pub user: SuspendingUser,
    pub dialog: Option<dialog::DialogInfo>,
    pub quarantined: bool,
    pub app_extras: Option<persistable::Bundle>,
    pub launcher_extras: Option<persistable::Bundle>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SuspendingUser {
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
        let mut suspension = Self {
            package,
            user: SuspendingUser::Persisted(e.int("suspending-user").ok().flatten()),
            dialog: None,
            quarantined: e.bool("quarantined").ok().flatten().unwrap_or(false),
            app_extras: None,
            launcher_extras: None,
        };
        while let Some(event) = cursor.next() {
            let parameter = match event {
                Event::End(_, d) if d <= depth => break,
                Event::Start(e, _) => e,
                _ => continue,
            };
            match parameter.name.as_str() {
                "dialog-info" => suspension.dialog = Some(dialog::DialogInfo::restore(parameter)),
                "app-extras" | "launcher-extras" => {
                    let bundle = match persistable::Bundle::read(cursor) {
                        Ok(bundle) => bundle,
                        // SuspendParams catches XmlPullParserException, preserving
                        // fields read before the failed bundle; runtime errors escape.
                        Err(persistable::Error::Xml(_)) => break,
                        Err(error) => return Err(error.to_string()),
                    };
                    if parameter.name == "app-extras" {
                        suspension.app_extras = Some(bundle);
                    } else {
                        suspension.launcher_extras = Some(bundle);
                    }
                }
                _ => {}
            }
        }
        Ok(suspension)
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
        for suspension in &self.suspensions {
            let owner = match suspension.user {
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
        resolved
    }

    pub fn is_quarantined(&self, user: i32, cross_user: bool) -> bool {
        self.resolved_suspensions(user, cross_user)
            .iter()
            .any(|(_, s)| s.quarantined)
    }
}

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
    pub icon_path: String,
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
                    r.packages.push((name, user_state(e)?));
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
        domain_verification_status: e.int("domainVerificationStatus")?.unwrap_or(0),
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
                    s.suspensions.push(Suspension::read(&mut cursor, by)?);
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
    if suspended && s.suspensions.is_empty() {
        let by = string(e, "suspending-package").unwrap_or_else(|| "android".into());
        if legacy_dialog.is_none() {
            legacy_dialog = string(e, "suspend_dialog_message")
                .filter(|m| !m.is_empty())
                .map(|message| dialog::DialogInfo {
                    message: Some(message),
                    ..dialog::DialogInfo::default()
                });
        }
        s.suspensions.push(Suspension {
            package: by,
            user: SuspendingUser::Current,
            dialog: legacy_dialog,
            quarantined: false,
            app_extras: legacy_app_extras,
            launcher_extras: legacy_launcher_extras,
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
    let activities: Vec<ArchiveActivity> = children(e, "archive-activity-info")
        .filter_map(|a| {
            Some(ArchiveActivity {
                title: string(a, "activity-title")?,
                original_component_name: string(a, "original-component-name")
                    .filter(|c| c.contains('/'))?,
                icon_path: string(a, "icon-path")?,
                monochrome_icon_path: string(a, "monochrome-icon-path"),
            })
        })
        .collect();
    let archive_time = e.long_hex("archive-time")?.unwrap_or(0);
    Ok(string(e, "installer-title")
        .filter(|_| !activities.is_empty())
        .map(|installer_title| ArchiveState {
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
        assert_eq!(s.suspensions.len(), 2);
        assert!(s.suspensions[0].app_extras.is_some());
        assert!(s.suspensions[1].launcher_extras.is_some());
        assert_eq!(
            s.resolved_suspensions(10, true)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            [0, 10]
        );
        assert!(s.is_quarantined(10, true));
        let resolved = s.resolved_suspensions(10, false);
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].1.dialog.as_ref().unwrap().title.as_deref(),
            Some("last")
        );
        assert!(!s.is_quarantined(10, false));
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
            suspension.dialog.as_ref().unwrap().title.as_deref(),
            Some("legacy")
        );
        assert_eq!(suspension.dialog.as_ref().unwrap().message, None);
        assert!(suspension.app_extras.is_some());
        assert!(!s.is_quarantined(10, true));
        let fallback = state(b"<pkg suspended='true' suspend_dialog_message='fallback'/>");
        assert_eq!(
            fallback.suspensions[0]
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
                .is_empty()
        );
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
            [0, 12, 0, 0]
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

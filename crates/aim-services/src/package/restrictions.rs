//! A user's package state, `/data/system/users/<user>/package-restrictions.xml`:
//! per package whether it is installed, enabled, stopped or hidden for the
//! user, its data directories' inodes, install reason and first install
//! time, and the components enabled or disabled at run time; the user's
//! default browser and the packages they may not uninstall. As
//! `Settings.readPackageRestrictionsLPr` reads it
//! (`writePackageRestrictions` writes it).
//!
//! Not modelled yet (#706): a suspension's parameters (dialog, extras)
//! beyond who suspended the package, preferred and persistent preferred
//! activities and cross-profile intent filters.

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
    pub ce_data_inode: i64,
    pub de_data_inode: i64,
    pub installed: bool,
    pub stopped: bool,
    pub not_launched: bool,
    pub hidden: bool,
    pub distraction_flags: i32,
    /// Who suspended the package: package and user (`None`: the user's
    /// own, or the platform's for a legacy `suspended` attribute).
    pub suspended_by: Vec<(String, Option<i32>)>,
    pub instant_app: bool,
    pub virtual_preload: bool,
    /// `COMPONENT_ENABLED_STATE_*`.
    pub enabled: i32,
    pub last_disable_app_caller: Option<String>,
    pub enabled_components: Vec<String>,
    pub disabled_components: Vec<String>,
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
    /// A package's state when the file is missing: installed, enabled, not
    /// stopped.
    fn default() -> Self {
        UserState {
            ce_data_inode: 0,
            de_data_inode: 0,
            installed: true,
            stopped: false,
            not_launched: false,
            hidden: false,
            distraction_flags: 0,
            suspended_by: Vec::new(),
            instant_app: false,
            virtual_preload: false,
            enabled: COMPONENT_ENABLED_STATE_DEFAULT,
            last_disable_app_caller: None,
            enabled_components: Vec::new(),
            disabled_components: Vec::new(),
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
        ..UserState::default()
    };
    for child in e.children() {
        match child.name.as_str() {
            "enabled-components" => s.enabled_components = components(child),
            "disabled-components" => s.disabled_components = components(child),
            "suspend-params" => {
                if let Some(by) = string(child, "suspending-package") {
                    s.suspended_by.push((by, child.int("suspending-user")?));
                }
            }
            "archive-state" => s.archive_state = archive_state(child)?,
            _ => {}
        }
    }
    if suspended && s.suspended_by.is_empty() {
        let by = string(e, "suspending-package").unwrap_or_else(|| "android".into());
        s.suspended_by.push((by, None));
    }
    Ok(s)
}

fn components(e: &Element) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in children(e, "item").filter_map(|i| string(i, "name")) {
        if !out.contains(&name) {
            out.push(name);
        }
    }
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

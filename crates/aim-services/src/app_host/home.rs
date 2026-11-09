//! The home activity of the default display, as `RootWindowContainer
//! .startHomeOnAllDisplays` starts it at `systemReady` at
//! `android-16.0.0_r1`: `startHomeOnTaskDisplayArea` resolves the home
//! intent (`ActivityTaskManagerService.getHomeIntent`, `resolveHomeActivity`)
//! and `ActivityStartController.startHomeActivity` starts it in the root
//! home task (`Stack::home`). In window mode that task stays behind the Mac
//! desktop, so its activity is launched to stopped and never gets a Mac
//! window; a HOME intent brings the desktop and the home to the front
//! (docs/windows.md).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::AppHost;
use super::activity::{ACTIVITY_TYPE_HOME, LaunchedFrom};
use super::activity_start::{LaunchIntent, StartRequest};
use super::kept::{KeptIntent, RawIntent};
use crate::package::intent::Intent;

const ACTION_MAIN: &str = "android.intent.action.MAIN";
const CATEGORY_HOME: &str = "android.intent.category.HOME";
const CATEGORY_SECONDARY_HOME: &str = "android.intent.category.SECONDARY_HOME";
/// `Intent.FLAG_DEBUG_TRIAGED_MISSING`, `FLAG_ACTIVITY_NEW_TASK`.
const FLAG_DEBUG_TRIAGED_MISSING: i32 = 0x100;
const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;
const SYSTEM_UID: i32 = 1000;
const ROOT_UID: i32 = 0;
/// The system user, the one the app host boots.
const USER: i32 = 0;

/// `getHomeIntent` of a normal boot: `ACTION_MAIN` with `CATEGORY_HOME`,
/// no data or component.
fn home_intent() -> Result<RawIntent, Exception> {
    RawIntent::new(ACTION_MAIN, FLAG_DEBUG_TRIAGED_MISSING, &[])
        .with_category(CATEGORY_HOME)
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("home intent: {e:?}")))
}

/// `ActivityRecord.isHomeIntent`: `ACTION_MAIN` with the one category
/// `CATEGORY_HOME` or `CATEGORY_SECONDARY_HOME`, no data or type.
fn is_home_intent(intent: &Intent) -> bool {
    intent.action.as_deref() == Some(ACTION_MAIN)
        && matches!(
            intent.categories.as_deref(),
            Some([c]) if c == CATEGORY_HOME || c == CATEGORY_SECONDARY_HOME
        )
        && intent.data.is_none()
        && intent.ty.is_none()
}

impl AppHost {
    /// `ActivityRecord.setActivityType` of `uid`'s start of `intent`, as
    /// the caller gave it: home of a home intent the caller left to
    /// resolution, or named a component of as one that may launch home
    /// (`canLaunchHomeActivity`: the system, root or the recents
    /// component), else unset. The resolver, the other way to home, is not
    /// started (#1485).
    pub(super) fn start_activity_type(&self, intent: &Intent, uid: i32) -> Result<i32, Exception> {
        if !is_home_intent(intent) {
            return Ok(0);
        }
        let allowed = intent.component.is_none()
            || matches!(uid, SYSTEM_UID | ROOT_UID)
            || self.recents_uid()?.is_some_and(|r| {
                r.rem_euclid(super::PER_USER_RANGE as i32)
                    == uid.rem_euclid(super::PER_USER_RANGE as i32)
            });
        Ok(if allowed { ACTIVITY_TYPE_HOME } else { 0 })
    }

    /// `startHomeOnTaskDisplayArea` of the default display: the home
    /// activity the package manager resolves for the system user, started
    /// as the system in the root home task.
    pub(super) fn start_home(self: &Arc<Self>) -> Result<(), Exception> {
        let intent = home_intent()?;
        // `resolveHomeActivity`: `ActivityTaskSupervisor.resolveIntent` by
        // the system, which no strict mode callback watches.
        let info = self
            .prepare_activity_start_filtered(SYSTEM_UID, 0, USER, &intent, None, SYSTEM_UID)?
            .info
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no home activity resolves"))?;
        let package = info
            .info
            .application_info
            .item
            .package_name
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "home activity of no package"))?;
        let class = info
            .info
            .item
            .name
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "home activity of no name"))?;
        // The intent names the home component, in a new task.
        let mut intent = intent.with_component(&package, &class);
        intent.set_flags(
            intent.intent.flags | FLAG_ACTIVITY_NEW_TASK,
            intent.intent.extended_flags,
        );
        eprintln!("app host: starting home {package}/{class}");
        let mut request = StartRequest::launcher(&class);
        request.intent = LaunchIntent::Raw(Box::new(KeptIntent::new(intent, &self.process)));
        request.referrer = "android".into();
        // `setCallingUid(0)`, with `ActivityOptions.setLaunchActivityType`.
        request.launched_from = LaunchedFrom {
            uid: 0,
            package: None,
            share_identity: false,
        };
        request.launch_options.activity_type = ACTIVITY_TYPE_HOME;
        self.start_in_new_task(&package, info, request)
    }
}

#[cfg(test)]
#[path = "home_tests.rs"]
mod tests;

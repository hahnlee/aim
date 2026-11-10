//! Background activity start verdicts, as `BackgroundActivityStartController
//! .checkBackgroundActivityStart` gives them at android-16.0.0_r1: the
//! caller's (a PendingIntent's creator's) and the real caller's (its
//! sender's) evaluated, then their opt-ins.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use aim_binder_host::parcel::Exception;

use super::AppHost;
use super::kept::RawBundle;

/// `BackgroundActivityStartController.BAL_*`, the reasons of a verdict.
pub(super) const BAL_BLOCK: i32 = 127;
pub(super) const BAL_ALLOW_ALLOWLISTED_UID: i32 = 2;
pub(super) const BAL_ALLOW_VISIBLE_WINDOW: i32 = 4;
pub(super) const BAL_ALLOW_PERMISSION: i32 = 6;

/// `ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_*`.
const MODE_SYSTEM_DEFINED: i32 = 0;
const MODE_ALLOWED: i32 = 1;
const MODE_DENIED: i32 = 2;
const MODE_ALLOW_ALWAYS: i32 = 3;
const MODE_ALLOW_IF_VISIBLE: i32 = 4;
const MODE_COMPAT: i32 = -1;

/// `ActivityOptions.KEY_PENDING_INTENT_CREATOR_BACKGROUND_ACTIVITY_START_MODE`
/// and `KEY_PENDING_INTENT_BACKGROUND_ACTIVITY_ALLOWED`.
pub(super) const KEY_CREATOR_MODE: &str =
    "android.activity.pendingIntentCreatorBackgroundActivityStartMode";
pub(super) const KEY_SENDER_MODE: &str = "android.pendingIntent.backgroundActivityAllowed";

/// `BackgroundActivityStartController.DEFAULT_RESCIND_BAL_PRIVILEGES_FROM_PENDING_INTENT_CREATOR`
/// and `PendingIntentRecord.DEFAULT_RESCIND_BAL_PRIVILEGES_FROM_PENDING_INTENT_SENDER`.
const RESCIND_FROM_CREATOR: i64 = 296_478_951;
const RESCIND_FROM_SENDER: i64 = 244_637_991;
/// `PendingIntent.PENDING_INTENT_OPTIONS_CHECK`.
const PENDING_INTENT_OPTIONS_CHECK: i64 = 320_664_730;

const ADDITIONAL_START_MODES: &str = "com.android.window.flags.bal_additional_start_modes";

/// `BalVerdict`: whether a background start is allowed and by what rule,
/// and whether only a PendingIntent's creator allows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct BalVerdict {
    pub code: i32,
    pub only_creator_allows: bool,
}

impl BalVerdict {
    const BLOCK: BalVerdict = BalVerdict::of(BAL_BLOCK);

    const fn of(code: i32) -> BalVerdict {
        BalVerdict {
            code,
            only_creator_allows: false,
        }
    }

    pub(super) fn allows(&self) -> bool {
        self.code != BAL_BLOCK
    }
}

/// A start's real caller (`ActivityStarter.Request.realCallingUid`).
#[derive(Clone, Copy)]
pub(super) struct RealCaller {
    pub uid: i32,
    /// `originatingPendingIntent`: the sender of a PendingIntent, whose
    /// creator is the caller.
    pub pending_intent: bool,
    /// A system start for the caller (`startActivityAsCaller`): its
    /// options allow the real caller's background starts by default
    /// (`createSafeActivityOptionsWithBalAllowed`), and a persistent
    /// system real caller allows it (`allowBalExemptionForSystemProcess`).
    pub bal_allowed: bool,
}

impl RealCaller {
    /// The `sender` of a PendingIntent, without exemptions
    /// (`getBackgroundStartPrivilegesForActivitySender` of no allowlist
    /// token).
    pub(super) fn pending_intent(sender: i32) -> RealCaller {
        RealCaller {
            uid: sender,
            pending_intent: true,
            bal_allowed: false,
        }
    }
}

impl AppHost {
    /// A PendingIntent's `options` from `uid` without the background
    /// start mode of `key`, which is the other party's to set (the
    /// sender's for its creator, `PendingIntentController.getIntentSender`,
    /// and the creator's for its sender, `PendingIntentRecord.sendInner`):
    /// refused if the options check applies to `uid`.
    pub(super) fn without_others_mode(
        &self,
        options: RawBundle,
        key: &str,
        uid: i32,
    ) -> Result<RawBundle, Exception> {
        let invalid = |_| Exception::illegal_argument("activity options");
        if options.int(key, MODE_SYSTEM_DEFINED).map_err(invalid)? == MODE_SYSTEM_DEFINED {
            return Ok(options);
        }
        eprintln!("app host: resetting {key} of a PendingIntent's options from uid {uid}");
        if self.change_enabled(PENDING_INTENT_OPTIONS_CHECK, uid)? {
            return Err(Exception::illegal_argument(format!(
                "{key} must not be set by uid {uid} for a PendingIntent"
            )));
        }
        options.without(key).map_err(invalid)
    }

    /// `checkBackgroundActivityStart` of a start by `uid`, with `real` its
    /// real caller if any, for a result if `for_result` (`resultRecord`),
    /// with `options`.
    pub(super) fn background_start_verdict(
        &self,
        uid: i32,
        real: Option<RealCaller>,
        for_result: bool,
        options: Option<&RawBundle>,
    ) -> Result<BalVerdict, Exception> {
        let mode = |key| {
            options
                .map_or(Ok(MODE_SYSTEM_DEFINED), |o| o.int(key, MODE_SYSTEM_DEFINED))
                .map_err(|_| Exception::illegal_argument("activity options"))
        };
        let creator = mode(KEY_CREATOR_MODE)?;
        let sender = match mode(KEY_SENDER_MODE)? {
            MODE_SYSTEM_DEFINED if real.is_some_and(|r| r.bal_allowed) => MODE_ALLOWED,
            sender => sender,
        };
        // `balAdditionalStartModes`: a mode allowing only if visible.
        let visible_only = |mode| -> Result<bool, Exception> {
            Ok(mode == MODE_ALLOW_IF_VISIBLE && self.flag(ADDITIONAL_START_MODES)?)
        };
        let pending_intent = real.is_some_and(|r| r.pending_intent);
        let mut caller = self.caller_verdict(uid, pending_intent, visible_only(creator)?)?;
        let Some(real) = real else {
            return Ok(caller);
        };
        let real_verdict = if uid == real.uid && caller.allows() {
            caller
        } else {
            self.real_caller_verdict(real, sender, visible_only(sender)?)?
        };
        caller.only_creator_allows = pending_intent && caller.allows() && !real_verdict.allows();
        // `BalState`: a start not of a PendingIntent opts both in, else one
        // for a result, of the same uid or of a compat sender opts its
        // sender in.
        let auto_sender = !pending_intent || for_result || uid == real.uid || sender == MODE_COMPAT;
        let opted_in = |mode| !matches!(mode, MODE_SYSTEM_DEFINED | MODE_DENIED);
        let caller_opt_in = if pending_intent {
            opted_in(creator)
        } else {
            creator != MODE_DENIED
        };
        let sender_opt_in = if auto_sender {
            sender != MODE_DENIED
        } else {
            opted_in(sender)
        };
        if caller.allows() && caller_opt_in {
            return Ok(caller);
        }
        if real_verdict.allows() && sender_opt_in {
            return Ok(real_verdict);
        }
        // Without an opt-in, a PendingIntent's creator's and sender's
        // privileges by default (`getBackgroundStartPrivilegesAllowedByCreator`,
        // `getDefaultBackgroundStartPrivileges`): none for root and system,
        // else unless their target SDK rescinds them.
        let by_default = |uid, change| -> Result<bool, Exception> {
            Ok(!matches!(uid, 0 | 1000) && !self.change_enabled(change, uid)?)
        };
        if caller.allows()
            && creator == MODE_SYSTEM_DEFINED
            && by_default(uid, RESCIND_FROM_CREATOR)?
        {
            return Ok(caller);
        }
        if real_verdict.allows()
            && sender == MODE_SYSTEM_DEFINED
            && by_default(real.uid, RESCIND_FROM_SENDER)?
        {
            return Ok(real_verdict);
        }
        Ok(BalVerdict::BLOCK)
    }

    /// `checkBackgroundActivityStartAllowedByCaller`: its visible window
    /// (only that if `visible_only`), else an allowlisted uid or its
    /// permission, a PendingIntent's creator's visibility last.
    fn caller_verdict(
        &self,
        uid: i32,
        pending_intent: bool,
        visible_only: bool,
    ) -> Result<BalVerdict, Exception> {
        let visible = || self.uid_visible(uid).then_some(BAL_ALLOW_VISIBLE_WINDOW);
        if visible_only {
            return Ok(visible().map_or(BalVerdict::BLOCK, BalVerdict::of));
        }
        let allowlisted = (uid == 0 || matches!(uid % super::PER_USER_RANGE as i32, 1000 | 1027))
            .then_some(BAL_ALLOW_ALLOWLISTED_UID);
        let permission = || -> Result<Option<i32>, Exception> {
            Ok((self.check_permission(
                Some("android.permission.START_ACTIVITIES_FROM_BACKGROUND"),
                uid,
            )? == 0)
                .then_some(BAL_ALLOW_PERMISSION))
        };
        let code = if pending_intent {
            match allowlisted {
                Some(code) => Some(code),
                None => permission()?.or_else(visible),
            }
        } else {
            match visible().or(allowlisted) {
                Some(code) => Some(code),
                None => permission()?,
            }
        };
        Ok(code.map_or(BalVerdict::BLOCK, BalVerdict::of))
    }

    /// `checkBackgroundActivityStartAllowedByRealCaller` in the `sender`
    /// mode: its visible window (only that if it allows only if visible),
    /// else its permission if it allows always, else a persistent system
    /// real caller if it allows always or the start exempts it.
    fn real_caller_verdict(
        &self,
        real: RealCaller,
        sender: i32,
        visible_only: bool,
    ) -> Result<BalVerdict, Exception> {
        if self.uid_visible(real.uid) {
            return Ok(BalVerdict::of(BAL_ALLOW_VISIBLE_WINDOW));
        }
        if visible_only {
            return Ok(BalVerdict::BLOCK);
        }
        let always = sender == MODE_ALLOW_ALWAYS;
        if always
            && self.check_permission(
                Some("android.permission.START_ACTIVITIES_FROM_BACKGROUND"),
                real.uid,
            )? == 0
        {
            return Ok(BalVerdict::of(BAL_ALLOW_PERMISSION));
        }
        let persistent = self
            .apps
            .lock()
            .unwrap()
            .values()
            .any(|a| a.uid as i32 == real.uid && a.proc_state <= 1);
        if (always || real.bal_allowed) && persistent {
            return Ok(BalVerdict::of(BAL_ALLOW_ALLOWLISTED_UID));
        }
        Ok(BalVerdict::BLOCK)
    }

    /// Whether `uid` has a visible window and app switches allow it
    /// (`mCheckCallerVisible`): a process of it runs.
    fn uid_visible(&self, uid: i32) -> bool {
        let running = self.uid_runs(uid);
        self.tasks.app_switches.lock().unwrap().allows(
            uid,
            running,
            false,
            std::time::Instant::now(),
        )
    }
}

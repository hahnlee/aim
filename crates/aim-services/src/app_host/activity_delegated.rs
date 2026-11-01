//! ATMS delegated starts, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AppHost,
    activity_start::{START_CANCELED, StartArgs},
    kept::{RawBundle, RawIntent},
    reply,
};
use aim_binder_host::{
    local::{Call, Reply},
    parcel::{BAD_VALUE, Binder, EX_UNSUPPORTED_OPERATION, Exception, Parcel},
};
use aim_service_aidl::{android_app_iactivitytaskmanager as atm, read_typed};
use std::sync::Arc;

pub(super) fn transact(host: &Arc<AppHost>, call: &mut Call<'_>) -> Reply {
    let r = &mut call.data;
    r.enforce_interface(atm::DESCRIPTOR)
        .map_err(|_| BAD_VALUE)?;
    r.read_binder().map_err(|_| BAD_VALUE)?; // IApplicationThread is unused by ATMS.
    // Android 16 has no permission-token argument. The activity token below
    // is mandatory; caller/package do not choose the delegated identity.
    r.read_string16().map_err(|_| BAD_VALUE)?;
    let args = StartArgs {
        caller: None,
        calling_package: None,
        intent: read_typed::<RawIntent>(r).map_err(|_| BAD_VALUE)?,
        resolved_type: r.read_string16().map_err(|_| BAD_VALUE)?,
        result_to: r.read_binder().map_err(|_| BAD_VALUE)?,
        result_who: r.read_string16().map_err(|_| BAD_VALUE)?,
        request_code: r.read_i32().map_err(|_| BAD_VALUE)?,
        options: None,
        launch: None,
    };
    let flags = r.read_i32().map_err(|_| BAD_VALUE)?;
    let profiler = read_typed::<super::activity::ProfilerInfo>(r).map_err(|_| BAD_VALUE)?;
    let args = StartArgs {
        options: read_typed::<RawBundle>(r).map_err(|_| BAD_VALUE)?,
        ..args
    };
    let ignore = r.read_bool().map_err(|_| BAD_VALUE)?;
    let user = r.read_i32().map_err(|_| BAD_VALUE)?;
    reply(
        host.start_as_caller(
            call,
            args,
            ignore,
            user,
            super::activity::unsupported_start(flags, profiler.is_some()),
        )
        .or_else(|e| {
            if e.code == EX_UNSUPPORTED_OPERATION {
                Ok(START_CANCELED)
            } else {
                Err(e)
            }
        })
        .map(|code| {
            let mut p = Parcel::new();
            atm::write_start_activity_reply(&mut p, code);
            p
        }),
    )
}

impl AppHost {
    fn start_as_caller(
        self: &Arc<Self>,
        call: &Call<'_>,
        mut args: StartArgs,
        ignore: bool,
        mut user: i32,
        unsupported: Option<&str>,
    ) -> Result<i32, Exception> {
        let _serial = self.activities.starts.lock().unwrap();
        let source = self.activities.with(args.result_to, |r| {
            (
                r.package.clone(),
                r.info.target_activity.as_ref().unwrap_or(&r.class).clone(),
                r.start_seq,
                r.launched_from.clone(),
            )
        });
        let (package, class, seq, from) = source.ok_or_else(|| {
            Exception::security("Must be called from an activity with a valid token")
        })?;
        let in_task = match args.result_to {
            Some(Binder::Local(t)) => {
                let stack = self.activities.stack.lock().unwrap();
                stack.get(t).is_some_and(|a| stack.task(a.task).is_some())
            }
            _ => false,
        };
        if !in_task {
            return Err(Exception::security("Called with bad activity token"));
        }
        let uid = self
            .apps
            .lock()
            .unwrap()
            .get(&seq)
            .filter(|a| a.thread.is_some())
            .map(|a| a.uid as i32)
            .ok_or_else(|| Exception::security("Called without a process attached to activity"))?;
        if self.check_permission(
            Some("android.permission.START_ACTIVITY_AS_CALLER"),
            call.sender_euid as i32,
        )? != 0
        {
            if package != "android" {
                return Err(Exception::security(
                    "Must be called from an activity that is declared in the android package",
                ));
            }
            if uid % super::PER_USER_RANGE as i32 != 1000 && uid != from.uid {
                return Err(Exception::security(
                    "Calling activity must be system uid or original calling uid",
                ));
            }
        }
        let intent = args
            .intent
            .as_ref()
            .ok_or_else(|| Exception::illegal_argument("intent"))?;
        if ignore && intent.intent.component.is_none() {
            return Err(Exception::security(
                "Component must be specified with ignoreTargetSecurity",
            ));
        }
        if ignore && intent.intent.selector.is_some() {
            return Err(Exception::security(
                "Selector not allowed with ignoreTargetSecurity",
            ));
        }
        if user == -10000 {
            user = uid / super::PER_USER_RANGE as i32;
        }
        if let Some(reason) = unsupported {
            return Ok(self.cancel_activity_start(reason));
        }
        let resolver = if package == "android" {
            let capture = self.package_scan_capture()?;
            let classes = capture.context().boot_classes.as_ref().ok_or_else(|| {
                self.unimplemented_named("activity_task", "boot class hierarchy (#2583)")
            })?;
            classes.assignable(
                &format!("L{};", class.replace('.', "/")),
                "Lcom/android/internal/app/ResolverActivity;",
            )
        } else {
            false
        };
        let mut prepared = self.prepare_activity_start_filtered(
            from.uid,
            -1,
            user,
            intent,
            args.resolved_type.as_deref(),
            if resolver { 0 } else { from.uid },
        )?;
        prepared.ignore_security = ignore;
        prepared.real_caller_uid = Some(call.sender_euid as i32);
        args.calling_package = from.package.clone();
        self.execute_activity_start(from.uid, from.package, -1, args, prepared)
            .map(|r| r.0)
    }

    // ATMS.createSafeActivityOptionsWithBalAllowed and BAL's two identities.
    pub(super) fn as_caller_background_allowed(
        &self,
        uid: i32,
        real_uid: i32,
        options: Option<&RawBundle>,
    ) -> Result<bool, Exception> {
        let mode = |key| {
            options
                .map_or(Ok(0), |o| o.int(key, 0))
                .map_err(|_| Exception::illegal_argument("activity options"))
        };
        let creator = mode("android.activity.pendingIntentCreatorBackgroundActivityStartMode")?;
        let sender = mode("android.pendingIntent.backgroundActivityAllowed")?;
        // SYSTEM_DEFINED is changed to ALLOWED by the ATMS wrapper.
        let sender = if sender == 0 { 1 } else { sender };
        let additional = self.flag("com.android.window.flags.bal_additional_start_modes")?;
        let foreground = |uid| {
            let running = self.uid_runs(uid);
            self.tasks.app_switches.lock().unwrap().allows(
                uid,
                running,
                false,
                std::time::Instant::now(),
            )
        };
        let caller_allowed = if additional && creator == 4 {
            foreground(uid)
        } else {
            self.background_activity_start_allowed(uid)?
        };
        if creator != 2 && caller_allowed {
            return Ok(true);
        }
        if sender == 2 {
            return Ok(false);
        }
        // The pinned controller reuses an allowed caller verdict for the same uid.
        if uid == real_uid && caller_allowed || foreground(real_uid) {
            return Ok(true);
        }
        if additional && sender == 4 {
            return Ok(false);
        }
        let persistent = self
            .apps
            .lock()
            .unwrap()
            .values()
            .any(|a| a.uid as i32 == real_uid && a.proc_state <= 1);
        Ok(persistent
            || sender == 3
                && self.check_permission(
                    Some("android.permission.START_ACTIVITIES_FROM_BACKGROUND"),
                    real_uid,
                )? == 0)
    }
}

#[cfg(test)]
#[path = "activity_delegated_tests.rs"]
mod tests;

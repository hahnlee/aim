//! ActivityStartController.startActivities and PendingIntentRecord.allIntents,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AppHost,
    activity_start::StartArgs,
    kept::{RawBundle, RawIntent},
    reply,
};
use aim_binder_host::{
    local::{Call, Reply},
    parcel::{
        BAD_VALUE, Binder, EX_NULL_POINTER, EX_SECURITY, EX_UNSUPPORTED_OPERATION, Exception,
        Parcel,
    },
};
use aim_service_aidl::{
    android_app_iactivitytaskmanager as atm, read_string_list, read_typed, read_typed_list,
};
use std::sync::Arc;

pub(super) fn transact(host: &Arc<AppHost>, call: &mut Call<'_>) -> Reply {
    let r = &mut call.data;
    r.enforce_interface(atm::DESCRIPTOR)
        .map_err(|_| BAD_VALUE)?;
    let caller = r.read_binder().map_err(|_| BAD_VALUE)?;
    let package = r.read_string16().map_err(|_| BAD_VALUE)?;
    r.read_string16().map_err(|_| BAD_VALUE)?;
    let intents = read_typed_list::<RawIntent>(r).map_err(|_| BAD_VALUE)?;
    let types = read_string_list(r).map_err(|_| BAD_VALUE)?;
    let args = StartArgs {
        caller: None,
        calling_package: package,
        intent: None,
        resolved_type: None,
        result_to: r.read_binder().map_err(|_| BAD_VALUE)?,
        result_who: None,
        request_code: -1,
        options: read_typed::<RawBundle>(r).map_err(|_| BAD_VALUE)?,
    };
    let user = r.read_i32().map_err(|_| BAD_VALUE)?;
    reply((|| {
        let Some((resolve_uid, pid, user)) = host.activity_start_caller(call, &args, Some(user))?
        else {
            unreachable!()
        };
        let intents = intents.ok_or_else(|| Exception::new(EX_NULL_POINTER, "intents is null"))?;
        let types =
            types.ok_or_else(|| Exception::new(EX_NULL_POINTER, "resolvedTypes is null"))?;
        if intents.len() != types.len() {
            return Err(Exception::illegal_argument(
                "intents are length different than resolvedTypes",
            ));
        }
        let code = host.start_activities_in_package(
            resolve_uid,
            pid,
            user,
            StartArgs { caller, ..args },
            intents,
            types,
        )?;
        let mut p = Parcel::new();
        atm::write_start_activity_reply(&mut p, code);
        Ok(p)
    })())
}

impl AppHost {
    pub(super) fn start_activities_in_package(
        self: &Arc<Self>,
        resolve_uid: i32,
        pid: i32,
        user: i32,
        mut args: StartArgs,
        intents: Vec<Option<RawIntent>>,
        types: Vec<Option<String>>,
    ) -> Result<i32, Exception> {
        args.request_code = -1;
        args.result_who = None;
        // AOSP filters null intents before indexing the resolved-type array.
        let mut prepared = Vec::new();
        for (i, mut intent) in intents.into_iter().flatten().enumerate() {
            intent.set_flags(intent.intent.flags, intent.intent.extended_flags & !1);
            let start = match self.prepare_activity_start(
                resolve_uid,
                pid,
                user,
                &intent,
                types[i].as_deref(),
            ) {
                Err(e) if e.code == EX_SECURITY => return Ok(-96),
                other => other?,
            };
            if start
                .info
                .as_ref()
                .is_some_and(|i| i.info.application_info.private_flags & 2 != 0)
            {
                return Err(Exception::illegal_argument(
                    "FLAG_CANT_SAVE_STATE not supported here",
                ));
            }
            prepared.push((intent, start));
        }
        let bottom = args
            .options
            .as_ref()
            .map(RawBundle::selective_launch_options)
            .transpose()
            .map_err(|_| Exception::illegal_argument("activity options"))?;
        let _serial = self.activities.starts.lock().unwrap();
        let source = args.result_to;
        let mut result_to = source;
        let mut new_task = false;
        let count = prepared.len();
        for (i, (mut intent, start)) in prepared.into_iter().enumerate() {
            let uid = if let Some(caller) = args.caller {
                let Some((_, _, uid, _)) = self.app_of_thread(Some(caller)) else {
                    return Ok(-94);
                };
                uid
            } else {
                resolve_uid
            };
            if new_task {
                intent.set_flags(
                    intent.intent.flags | super::activity_stack::FLAG_ACTIVITY_NEW_TASK,
                    intent.intent.extended_flags,
                );
            }
            let result = self.execute_activity_start(
                uid,
                args.calling_package.clone(),
                pid,
                StartArgs {
                    intent: Some(intent),
                    result_to,
                    options: if i + 1 == count {
                        args.options.clone()
                    } else {
                        bottom.clone()
                    },
                    ..args.clone()
                },
                start,
            );
            let (code, token) = match result {
                Err(e) if e.code == EX_UNSUPPORTED_OPERATION => return Ok(-96),
                other => other?,
            };
            if code < 0 {
                return Ok(code);
            }
            if token.is_some_and(|t| {
                self.activities
                    .with(Some(Binder::Local(t)), |r| r.uid as i32 == resolve_uid)
                    .unwrap_or(false)
            }) {
                result_to = token.map(Binder::Local);
                new_task = false;
            } else {
                result_to = source;
                new_task = true;
            }
        }
        Ok(0)
    }
}

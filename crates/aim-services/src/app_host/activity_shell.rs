//! ActivityManagerShellCommand, android-16.0.0_r1 (AOSP Apache-2.0).
//! Original am.jar owns Instrument's parser and watcher. Only syntax and
//! display strings cross the Java leaf; native owners retain caller policy.
use std::sync::mpsc;
use std::time::Instant;

use aim_binder_host::local::{Call, Reply};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_service_aidl::{
    android_app_iactivitymanager as am, dev_aim_server_ipackageintentparser as syntax,
};

use super::kept::{RawBundle, RawIntent};
use super::{AppHost, reply};
use crate::shell::ShellCommand;

pub(super) fn transact(host: &AppHost, call: &mut Call<'_>) -> Reply {
    if !matches!(call.sender_euid, 0 | 2000) {
        return reply(Err(Exception::security(
            "Shell commands are only callable by ADB",
        )));
    }
    let mut command = ShellCommand::read(&host.process, &mut call.data).map_err(|_| BAD_VALUE)?;
    if command.has_output() {
        let status = match run(host, call.sender_euid as i32, call.sender_pid, &mut command) {
            Ok(status) => status,
            Err(error) => {
                let class = match error.code {
                    aim_binder_host::parcel::EX_SECURITY => "java.lang.SecurityException",
                    aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT => {
                        "java.lang.IllegalArgumentException"
                    }
                    _ => "java.lang.IllegalStateException",
                };
                command.exception(&format!("{class}: {}", error.message));
                -1
            }
        };
        command.finish(status);
    }
    reply(Ok({
        let mut p = Parcel::new();
        p.write_no_exception();
        p
    }))
}

fn failure(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
fn codec(code: i32) -> Exception {
    failure(format!("activity shell codec: {code}"))
}

fn leaf(host: &AppHost, code: u32, request: Parcel) -> Result<Parcel, Exception> {
    let owner = host
        .package_intent_parser
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| failure("original Intent parser unavailable"))?;
    owner
        .transact_as_owner(code, &request, false)
        .map(|r| r.into_parcel())
        .map_err(codec)
}

fn run(host: &AppHost, uid: i32, pid: i32, c: &mut ShellCommand) -> Result<i32, Exception> {
    match c.command().unwrap_or_default() {
        "instrument" => {
            c.eprintln("Error: must be invoked through 'am instrument'.");
            Ok(-1)
        }
        "start" | "start-activity" => start(host, uid, pid, c),
        "broadcast" => broadcast(host, uid, pid, c),
        "wait-for-broadcast-idle" | "wait-for-broadcast-barrier" => {
            let barrier = c.command() == Some("wait-for-broadcast-barrier");
            if let Some(option) = c.next_arg() {
                c.eprintln(&format!(
                    "Error: unsupported activity shell option '{option}' (#2511)"
                ));
                return Ok(-1);
            }
            if host.check_permission(Some("android.permission.DUMP"), uid)? != 0 {
                return Err(Exception::security(
                    "Permission Denial: waitForBroadcast requires android.permission.DUMP",
                ));
            }
            host.broadcasts.wait(barrier);
            if barrier {
                c.println("Test barrier passed");
            } else {
                c.println("Test idle passed");
                c.println("All broadcast queues are idle!");
            }
            Ok(0)
        }
        _ => {
            c.eprintln(&format!(
                "Error: unsupported activity shell command '{}' (#2511)",
                c.command().unwrap_or_default()
            ));
            Ok(-1)
        }
    }
}

fn broadcast(host: &AppHost, uid: i32, pid: i32, c: &mut ShellCommand) -> Result<i32, Exception> {
    let mut request = Parcel::new();
    syntax::ParseBroadcast {
        arguments: Some(c.args[1..].iter().cloned().map(Some).collect()),
    }
    .write(&mut request);
    let parsed = leaf(host, syntax::PARSE_BROADCAST, request)?;
    let bytes = syntax::read_parse_broadcast_reply(&mut parsed.reader())
        .map_err(codec)??
        .ok_or_else(|| failure("null broadcast syntax"))?;
    let mut r = Reader::new(&bytes, &[]);
    let user_id = r.read_i32().map_err(codec)?;
    let permission = r.read_string16().map_err(codec)?;
    let display = r.read_string16().map_err(codec)?.unwrap_or_default();
    let intent = RawIntent::read_from(&mut r).map_err(codec)?;
    if r.remaining() != 0 {
        return Err(failure("broadcast syntax trailing data"));
    }
    c.println(&format!("Broadcasting: {display}"));
    let (send, receive) = mpsc::channel();
    let result = host.broadcasts.broadcast_with_result(
        host,
        uid,
        pid,
        None,
        None,
        am::BroadcastIntentWithFeature {
            caller: None,
            calling_feature_id: None,
            intent: Some(intent),
            resolved_type: None,
            result_to: None,
            result_code: 0,
            result_data: None,
            map: None,
            required_permissions: permission.map(|p| vec![Some(p)]),
            exclude_permissions: None,
            exclude_packages: None,
            app_op: -1,
            options: None,
            serialized: true,
            sticky: false,
            user_id,
        },
        Some(Box::new(move |code, data, extras| {
            let mut p = Parcel::new();
            p.write_i32(code);
            p.write_string16(data.as_deref());
            match extras {
                Some(e) => e.bundle.write_to(&mut p),
                None => p.write_i32(-1),
            }
            let _ = send.send(p.data().to_vec());
        })),
    )?;
    if result == 0 {
        let result = receive.recv().map_err(|e| failure(e.to_string()))?;
        let mut request = Parcel::new();
        syntax::FormatBroadcastResult {
            result: Some(result),
        }
        .write(&mut request);
        let formatted = leaf(host, syntax::FORMAT_BROADCAST_RESULT, request)?;
        let line = syntax::read_format_broadcast_result_reply(&mut formatted.reader())
            .map_err(codec)??;
        c.println(line.as_deref().unwrap_or_default());
    }
    Ok(0)
}

// ActivityManagerShellCommand.runStartActivity, android-16.0.0_r1:
// original Intent syntax/presentation, native starter and caller identity.
fn start(host: &AppHost, uid: i32, pid: i32, c: &mut ShellCommand) -> Result<i32, Exception> {
    let mut request = Parcel::new();
    syntax::ParseActivityStart {
        arguments: Some(c.args[1..].iter().cloned().map(Some).collect()),
    }
    .write(&mut request);
    let parsed = leaf(host, syntax::PARSE_ACTIVITY_START, request)?;
    let bytes = syntax::read_parse_activity_start_reply(&mut parsed.reader())
        .map_err(codec)??
        .ok_or_else(|| failure("null activity start syntax"))?;
    let mut r = Reader::new(&bytes, &[]);
    let user = r.read_i32().map_err(codec)?;
    let wait = r.read_bool().map_err(codec)?;
    let display = r.read_string16().map_err(codec)?.unwrap_or_default();
    let intent = RawIntent::read_from(&mut r).map_err(codec)?;
    let options = aim_service_aidl::read_typed::<RawBundle>(&mut r).map_err(codec)?;
    if r.remaining() != 0 {
        return Err(failure("activity start syntax trailing data"));
    }
    if user == -1 {
        c.eprintln("Error: Can't start service with user 'all'");
        return Ok(1);
    }
    host.activity_launch_options(options.as_ref(), uid)?;
    let resolved_type = match (&intent.intent.ty, &intent.intent.data) {
        (Some(ty), _) => Some(ty.clone()),
        (None, Some(uri)) => host.provider_type(uri, uid)?,
        _ => None,
    };
    c.println(&format!("Starting: {display}"));
    let component = intent.intent.component.clone();
    let empty = Parcel::new();
    let call = Call {
        code: aim_service_aidl::android_app_iactivitytaskmanager::START_ACTIVITY_AS_USER,
        flags: 0,
        sender_euid: uid as u32,
        sender_pid: pid,
        data: empty.reader(),
    };
    let started = Instant::now();
    let host = host
        .me
        .upgrade()
        .ok_or_else(|| failure("activity owner gone"))?;
    let args = super::activity_start::StartArgs {
        caller: None,
        calling_package: Some("com.android.shell".into()),
        intent: Some(intent),
        resolved_type,
        result_to: None,
        result_who: None,
        request_code: 0,
        options,
        start_flags: 0,
        launch: None,
    };
    let waited = if wait {
        Some(host.wait_activity_start(&call, args.clone(), user, None)?)
    } else {
        None
    };
    let result = if let Some(w) = &waited {
        w.result
    } else {
        host.start_activity(&call, args, Some(user), None, None)?
    };
    let message = match result {
        0 => String::new(),
        2 => "Warning: Activity not started, its current task has been brought to the front".into(),
        3 => "Warning: Activity not started, intent has been delivered to currently running top-most instance".into(),
        -1 => format!("Error: Activity not started, unable to resolve {display}"),
        -2 => {
            if wait { c.println("Error type 3"); } else { c.eprintln("Error type 3"); }
            let name = component.map(|v| format!("{{{}}}", super::services::short_name(&v.package, &v.class))).unwrap_or_default();
            format!("Error: Activity class {name} does not exist.")
        }
        -93 => "Error: Activity not started, you requested to both forward and receive its result".into(),
        -94 => "Error: Activity not started, you do not have permission to access it.".into(),
        _ => format!("Error: Activity not started, unknown error code {result}"),
    };
    if !message.is_empty() {
        if wait {
            c.println(&message);
        } else {
            c.eprintln(&message);
        }
    }
    if matches!(result, 0 | 2 | 3) {
        if let Some(w) = waited {
            c.println(&w.shell_output(started.elapsed().as_millis()));
        }
        Ok(0)
    } else {
        Ok(1)
    }
}

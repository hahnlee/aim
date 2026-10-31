//! ActivityManagerShellCommand, android-16.0.0_r1 (AOSP Apache-2.0).
//! Original am.jar owns Instrument's parser and watcher. Only syntax and
//! display strings cross the Java leaf; native owners retain caller policy.
use std::sync::mpsc;

use aim_binder_host::local::{Call, Reply};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_service_aidl::{
    android_app_iactivitymanager as am, dev_aim_server_ipackageintentparser as syntax,
};

use super::kept::RawIntent;
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

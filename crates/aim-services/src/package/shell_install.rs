//! PackageManagerShellCommand installation sessions at android-16.0.0_r1.
//! AOSP, Apache License 2.0. All mutations use the public native Binder owners.
use super::{
    installer::{
        codec::{SessionInfo, SessionParams},
        preapproval::IntentSender,
    },
    shell::Context,
};
use aim_binder_host::{
    local::{Call, Service},
    parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader},
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, android_content_iintentsender as sender,
    android_content_pm_ipackageinstaller as installer,
    android_content_pm_ipackageinstallersession as session,
    android_content_pm_ipackagemanager as pm,
};
use std::{
    collections::VecDeque,
    os::fd::AsFd,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
fn bad(message: impl Into<String>) -> Exception {
    Exception::illegal_argument(message)
}
fn wire(status: i32) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, format!("package shell parcel: {status}"))
}
fn checked(reply: Parcel) -> Result<Parcel, Exception> {
    reply.reader().read_exception().map_err(wire)??;
    Ok(reply)
}
fn integer<T: std::str::FromStr>(text: &str) -> Result<T, Exception> {
    text.parse()
        .map_err(|_| bad(format!("Invalid number: {text}")))
}
fn arg(args: &[String], at: &mut usize) -> Result<String, Exception> {
    let value = args
        .get(*at)
        .cloned()
        .ok_or_else(|| bad("Argument expected"))?;
    *at += 1;
    Ok(value)
}
fn user(text: &str) -> Result<i32, Exception> {
    match text {
        "all" => Ok(-1),
        "current" | "cur" => Ok(-2),
        _ => integer(text),
    }
}
fn uri_object(value: String) -> super::installer::codec::Object {
    let mut p = Parcel::new();
    p.write_string16(Some("android.net.Uri$StringUri"));
    p.write_i32(1);
    p.write_string8(Some(&value));
    super::installer::codec::Object {
        bytes: p.data().to_vec(),
        objects: p.objects().to_vec(),
    }
}
fn installer(ctx: &Context<'_>) -> Result<Binder, Exception> {
    let mut p = Parcel::new();
    pm::GetPackageInstaller {}.write(&mut p);
    pm::read_get_package_installer_reply(
        &mut ctx.invoke_public(pm::GET_PACKAGE_INSTALLER, p)?.reader(),
    )
    .map_err(wire)??
    .ok_or_else(|| wire(BAD_VALUE))
}
fn open(ctx: &Context<'_>, owner: Binder, id: i32) -> Result<Binder, Exception> {
    let mut p = Parcel::new();
    installer::OpenSession { session_id: id }.write(&mut p);
    installer::read_open_session_reply(&mut ctx.invoke(owner, installer::OPEN_SESSION, p)?.reader())
        .map_err(wire)??
        .ok_or_else(|| wire(BAD_VALUE))
}
fn close(ctx: &Context<'_>, target: Binder) -> Result<(), Exception> {
    let mut p = Parcel::new();
    session::Close {}.write(&mut p);
    checked(ctx.invoke(target, session::CLOSE, p)?)?;
    Ok(())
}
fn with_session<T>(
    ctx: &mut Context<'_>,
    owner: Binder,
    id: i32,
    f: impl FnOnce(&mut Context<'_>, Binder) -> Result<T, Exception>,
) -> Result<T, Exception> {
    let target = open(ctx, owner, id)?;
    let result = f(ctx, target);
    let closed = close(ctx, target);
    match result {
        Err(error) => Err(error),
        Ok(value) => {
            closed?;
            Ok(value)
        }
    }
}
fn info(ctx: &Context<'_>, owner: Binder, id: i32) -> Result<Option<SessionInfo>, Exception> {
    let mut p = Parcel::new();
    installer::GetSessionInfo { session_id: id }.write(&mut p);
    installer::read_get_session_info_reply(
        &mut ctx.invoke(owner, installer::GET_SESSION_INFO, p)?.reader(),
    )
    .map_err(wire)?
}
struct Params {
    session: SessionParams,
    installer: Option<String>,
    user: i32,
    timeout: i64,
    paths: Vec<String>,
}
fn params(ctx: &Context<'_>, args: &[String]) -> Result<Params, Exception> {
    let mut p = Params {
        session: SessionParams {
            mode: 1,
            install_flags: 0x400000,
            install_location: 1,
            size_bytes: -1,
            originating_uid: -1,
            auto_revoke_permissions_mode: 3,
            required_installed_version_code: -1,
            unarchive_id: -1,
            package_source: 1,
            auto_install_dependencies_enabled: true,
            ..Default::default()
        },
        installer: None,
        user: -1,
        timeout: 60000,
        paths: Vec::new(),
    };
    let mut at = 0;
    let mut replace = true;
    let mut staged = None;
    let mut force = false;
    while at < args.len() {
        let opt = arg(args, &mut at)?;
        if !opt.starts_with('-') || opt == "-" {
            p.paths.push(opt);
            p.paths.extend_from_slice(&args[at..]);
            break;
        }
        match opt.as_str() {
            "-r" | "--force-sdk" => {}
            "-R" => replace = false,
            "-i" => p.installer = Some(arg(args, &mut at)?),
            "-t" => p.session.install_flags |= 4,
            "-f" => p.session.install_flags |= 0x10,
            "-d" => p.session.install_flags |= 0x80,
            "-g" => p.session.install_flags |= 0x100,
            "--restrict-permissions" => p.session.install_flags &= !0x400000,
            "--dont-kill" => p.session.install_flags |= 0x1000,
            "-p" => {
                p.session.mode = 2;
                p.session.app_package_name = Some(arg(args, &mut at)?);
            }
            "--pkg" => p.session.app_package_name = Some(arg(args, &mut at)?),
            "-S" => {
                p.session.size_bytes = integer(&arg(args, &mut at)?)?;
                if p.session.size_bytes <= 0 {
                    return Err(bad("Size must be positive"));
                }
            }
            "--originating-uri" => {
                p.session.originating_uri = Some(uri_object(arg(args, &mut at)?))
            }
            "--referrer" => p.session.referrer_uri = Some(uri_object(arg(args, &mut at)?)),
            "--abi" => {
                let abi = arg(args, &mut at)?;
                ctx.validate_install_abi(&abi)?;
                p.session.abi_override = Some(abi);
            }
            "--user" => p.user = user(&arg(args, &mut at)?)?,
            "--install-location" => p.session.install_location = integer(&arg(args, &mut at)?)?,
            "--install-reason" => p.session.install_reason = integer(&arg(args, &mut at)?)?,
            "--ephemeral" | "--instant" | "--instantapp" => {
                p.session.install_flags |= 0x800;
                p.session.install_flags &= !0x4000;
            }
            "--full" => {
                p.session.install_flags |= 0x4000;
                p.session.install_flags &= !0x800;
            }
            "--preload" => p.session.install_flags |= 0x10000,
            "--force-uuid" => {
                p.session.install_flags |= 0x200;
                let v = arg(args, &mut at)?;
                p.session.volume_uuid = (v != "internal").then_some(v);
            }
            "--apex" => p.session.install_flags |= 0x20000,
            "--multi-package" => p.session.multi_package = true,
            "--staged" => staged = Some(true),
            "--non-staged" => staged = Some(false),
            "--force-non-staged" => force = true,
            "--force-queryable" => p.session.force_queryable_override = true,
            "--skip-verification" => p.session.install_flags |= 0x80000,
            "--skip-enable" => p.session.application_enabled_setting_persistent = true,
            "--bypass-low-target-sdk-block" => p.session.install_flags |= 0x1000000,
            "--ignore-dexopt-profile" => p.session.install_flags |= 1 << 28,
            "--update-ownership" => {
                p.session.install_flags |= 1 << 25;
                p.installer.get_or_insert("com.android.shell".into());
            }
            "--enable-rollback" => {
                p.session.install_flags |= 0x40000;
                p.installer.get_or_insert("com.android.shell".into());
                if let Some(v) = args.get(at).and_then(|v| v.parse::<i32>().ok()) {
                    if !(0..=2).contains(&v) {
                        return Err(bad("Invalid rollback data policy"));
                    }
                    p.session.rollback_data_policy = v;
                    at += 1;
                }
            }
            "--rollback-impact-level" => {
                p.session.rollback_impact_level = integer(&arg(args, &mut at)?)?;
                if !(0..=2).contains(&p.session.rollback_impact_level) {
                    return Err(bad("Invalid rollback impact level"));
                }
            }
            "--dexopt-compiler-filter" => {
                let filter = arg(args, &mut at)?;
                ctx.validate_install_compiler_filter(&filter)?;
                p.session.dexopt_compiler_filter = Some(filter)
            }
            "--disable-auto-install-dependencies" => {
                if !ctx.sdk_dependency_installer_enabled()? {
                    return Err(bad(format!("Unknown option {opt}")));
                }
                p.session.auto_install_dependencies_enabled = false;
            }
            "--staged-ready-timeout" => p.timeout = integer(&arg(args, &mut at)?)?,
            "--package-source" => p.session.package_source = integer(&arg(args, &mut at)?)?,
            _ => return Err(bad(format!("Unknown option {opt}"))),
        }
    }
    if replace {
        p.session.install_flags |= 2;
    }
    p.session.staged = !force && staged.unwrap_or(p.session.install_flags & 0x20000 != 0);
    if force {
        p.session.development_install_flags |= 1;
    }
    if p.session.install_flags & 0x60000 == 0x60000 && p.session.rollback_data_policy == 1 {
        return Err(bad("Data policy 'wipe' is not supported for apex."));
    }
    Ok(p)
}
fn create(ctx: &Context<'_>, owner: Binder, params: &mut Params) -> Result<i32, Exception> {
    if params.user == -1 {
        params.session.install_flags |= 0x40;
    }
    if let Some(abi) = params.session.abi_override.as_deref() {
        ctx.validate_install_abi(abi)?;
    }
    let translated = ctx.translate_user(params.user, 0, "doCreateSession")?;
    let mut p = Parcel::new();
    installer::CreateSession {
        params: Some(params.session.clone()),
        installer_package_name: params.installer.clone(),
        installer_attribution_tag: None,
        user_id: translated,
    }
    .write(&mut p);
    installer::read_create_session_reply(
        &mut ctx.invoke(owner, installer::CREATE_SESSION, p)?.reader(),
    )
    .map_err(wire)?
}
struct Fd(aim_binder_driver::File);
impl WriteParcelable for Fd {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}
fn write(
    ctx: &mut Context<'_>,
    owner: Binder,
    id: i32,
    name: String,
    path: Option<&str>,
    mut size: i64,
    log: bool,
) -> Result<i32, Exception> {
    with_session(ctx, owner, id, |ctx, target| {
        let file = ctx.install_input(path)?;
        if path.is_some_and(|p| p != "-") {
            let metadata = file
                .metadata()
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))?;
            if !metadata.is_file() {
                ctx.command
                    .eprintln(&format!("Unable to get size of: {}", path.unwrap()));
                return Ok(-1);
            }
            size = metadata.len() as i64;
        }
        if size <= 0 {
            ctx.command.eprintln("Error: must specify an APK size");
            return Ok(1);
        }
        let fd = file.into_file_owner();
        let mut p = Parcel::new();
        session::Write {
            name: Some(name),
            offset_bytes: 0,
            length_bytes: size,
            fd: Some(Fd(fd)),
        }
        .write(&mut p);
        checked(ctx.invoke(target, session::WRITE, p)?)?;
        if log {
            ctx.command
                .println(&format!("Success: streamed {size} bytes"));
        }
        Ok(0)
    })
}
#[derive(Default)]
struct ResultState {
    result: Mutex<VecDeque<Result<InstallResult, i32>>>,
    changed: Condvar,
}
struct InstallResult {
    status: i32,
    message: Option<String>,
    warnings: Vec<String>,
}
impl ResultState {
    fn wait(&self) -> Result<InstallResult, Exception> {
        let mut state = self.result.lock().unwrap();
        while state.is_empty() {
            state = self.changed.wait(state).unwrap();
        }
        state.pop_front().unwrap().map_err(wire)
    }
}
struct Receiver(Arc<ResultState>);
fn result_intent(r: &mut Reader<'_>) -> Result<InstallResult, i32> {
    r.read_string8()?;
    super::uri::Uri::read(r, &mut super::intent_filter::Plain)?;
    r.read_string8()?;
    r.read_string8()?;
    r.read_i32()?;
    r.read_i32()?;
    r.read_string8()?;
    if r.read_string16()?.is_some() {
        r.read_string16()?;
    }
    if r.read_i32()? != 0 {
        r.skip(16)?;
    }
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string8()?;
    }
    if r.read_i32()? != 0 {
        super::intent::Intent::read(r, &mut super::intent_filter::Plain)?;
    }
    if r.read_i32()? != 0 {
        crate::clip::ClipData::read_from(r)?;
    }
    r.read_i32()?;
    let extras = crate::bundle::read(r)?.unwrap_or_default();
    let status = match extras.get("android.content.pm.extra.STATUS") {
        Some(crate::bundle::Value::Int(v)) => *v,
        _ => 1,
    };
    let message = match extras.get("android.content.pm.extra.STATUS_MESSAGE") {
        Some(crate::bundle::Value::String(v)) => v.clone(),
        _ => None,
    };
    let mut warnings = Vec::new();
    if let Some(crate::bundle::Value::Lazy {
        kind: 11,
        start,
        len,
    }) = extras.get("android.content.pm.extra.WARNINGS")
    {
        let saved = r.position();
        r.set_position(*start);
        let end = start.checked_add(*len).ok_or(BAD_VALUE)?;
        for _ in 0..r.read_i32()?.max(0) {
            if r.read_i32()? != 0 {
                return Err(BAD_VALUE);
            }
            if let Some(v) = r.read_string16()? {
                warnings.push(v);
            }
        }
        if r.position() != end {
            return Err(BAD_VALUE);
        }
        r.set_position(saved);
    }
    if r.read_i32()? != 0 {
        super::intent::Intent::read(r, &mut super::intent_filter::Plain)?;
    }
    if r.read_i32()? != 0 {
        r.read_binder()?;
        for _ in 0..r.read_i32()?.max(0) {
            r.read_i32()?;
            r.read_string8()?;
            r.read_i32()?;
        }
    }
    Ok(InstallResult {
        status,
        message,
        warnings,
    })
}
impl Service for Receiver {
    fn descriptor(&self) -> &str {
        sender::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> aim_binder_host::local::Reply {
        if call.code != sender::SEND {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        let result = (|| {
            call.data.enforce_interface(sender::DESCRIPTOR)?;
            call.data.read_i32()?;
            if call.data.read_i32()? == 0 {
                return Err(BAD_VALUE);
            }
            let result = result_intent(&mut call.data)?;
            call.data.read_string16()?;
            call.data.read_binder()?;
            call.data.read_binder()?;
            call.data.read_string16()?;
            if call.data.read_i32()? != 0 {
                crate::clip::bundle(&mut call.data)?;
            }
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            Ok(result)
        })();
        self.0.result.lock().unwrap().push_back(result);
        self.0.changed.notify_all();
        Ok(Parcel::new())
    }
}
fn report(ctx: &mut Context<'_>, result: InstallResult) -> i32 {
    if result.status == 0 {
        if result.warnings.is_empty() {
            ctx.command.println("Success");
            0
        } else {
            for w in result.warnings {
                ctx.command.println(&format!("Warning: {w}"));
            }
            ctx.command.println("Completed with warning(s)");
            1
        }
    } else {
        ctx.command.println(&format!(
            "Failure [{}]",
            result.message.as_deref().unwrap_or("null")
        ));
        1
    }
}
fn commit(ctx: &mut Context<'_>, owner: Binder, id: i32, timeout: i64) -> Result<i32, Exception> {
    let state = Arc::new(ResultState::default());
    let binder = ctx.publish_receiver(Arc::new(Receiver(state.clone())))?;
    let result = with_session(ctx, owner, id, |ctx, target| {
        let mut p = Parcel::new();
        session::IsStaged {}.write(&mut p);
        let staged =
            session::read_is_staged_reply(&mut ctx.invoke(target, session::IS_STAGED, p)?.reader())
                .map_err(wire)??;
        let mut p = Parcel::new();
        session::IsMultiPackage {}.write(&mut p);
        let multi = session::read_is_multi_package_reply(
            &mut ctx.invoke(target, session::IS_MULTI_PACKAGE, p)?.reader(),
        )
        .map_err(wire)??;
        if !staged && !multi {
            let mut p = Parcel::new();
            session::GetNames {}.write(&mut p);
            let names = session::read_get_names_reply(
                &mut ctx.invoke(target, session::GET_NAMES, p)?.reader(),
            )
            .map_err(wire)??
            .unwrap_or_default();
            let unmatched: Vec<_> = names
                .iter()
                .flatten()
                .filter(|name| {
                    name.ends_with(".dm")
                        && !names.iter().flatten().any(|apk| {
                            apk.strip_suffix(".apk")
                                .is_some_and(|base| format!("{base}.dm") == **name)
                        })
                })
                .collect();
            if !unmatched.is_empty() {
                ctx.command.println(&format!(
                    "Warning [Could not validate the dex paths: Unmatched .dm files: [{}]]",
                    unmatched
                        .iter()
                        .map(|name| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let mut p = Parcel::new();
        session::Commit {
            status_receiver: Some(IntentSender { target: binder }),
            for_transferred: false,
        }
        .write(&mut p);
        checked(ctx.invoke(target, session::COMMIT, p)?)?;
        if staged {
            if timeout > 0 {
                let started = Instant::now();
                loop {
                    let Some(s) = info(ctx, owner, id)? else {
                        ctx.command
                            .println("Failure [failed to retrieve SessionInfo]");
                        return Ok(1);
                    };
                    if s.session_ready {
                        break;
                    }
                    if s.session_failed {
                        ctx.command.println(&format!(
                            "Error [{}] [{}]",
                            s.session_error_code,
                            s.session_error_message.as_deref().unwrap_or("null")
                        ));
                        return Ok(1);
                    }
                    if started.elapsed() >= Duration::from_millis(timeout as u64) {
                        ctx.command
                            .println(&format!("Failure [timed out after {timeout} ms]"));
                        return Ok(1);
                    }
                    std::thread::sleep(Duration::from_millis(100).min(
                        Duration::from_millis(timeout as u64).saturating_sub(started.elapsed()),
                    ));
                }
            }
            ctx.command.println(if timeout > 0 {
                "Success. Reboot device to apply staged session"
            } else {
                "Success"
            });
            Ok(0)
        } else {
            Ok(report(ctx, state.wait()?))
        }
    });
    ctx.retire_receiver(binder);
    result
}
fn abandon(ctx: &mut Context<'_>, owner: Binder, id: i32, log: bool) -> Result<i32, Exception> {
    with_session(ctx, owner, id, |ctx, target| {
        let mut p = Parcel::new();
        session::Abandon {}.write(&mut p);
        checked(ctx.invoke(target, session::ABANDON, p)?)?;
        if log {
            ctx.command.println("Success");
        }
        Ok(0)
    })
}
fn remove(
    ctx: &mut Context<'_>,
    owner: Binder,
    id: i32,
    names: &[String],
    log: bool,
) -> Result<i32, Exception> {
    with_session(ctx, owner, id, |ctx, target| {
        for name in names {
            let mut p = Parcel::new();
            session::RemoveSplit {
                split_name: Some(name.clone()),
            }
            .write(&mut p);
            checked(ctx.invoke(target, session::REMOVE_SPLIT, p)?)?;
        }
        if log {
            ctx.command.println("Success");
        }
        Ok(0)
    })
}
fn staged_list(ctx: &mut Context<'_>, owner: Binder, args: &[String]) -> Result<i32, Exception> {
    let (mut only_parent, mut only_ready, mut only_id) = (false, false, false);
    for opt in args {
        match opt.as_str() {
            "--only-parent" => only_parent = true,
            "--only-ready" => only_ready = true,
            "--only-sessionid" => only_id = true,
            _ => {
                ctx.command
                    .println(&format!("Error: Unknown option: {opt}"));
                return Ok(-1);
            }
        }
    }
    let mut p = Parcel::new();
    installer::GetStagedSessions {}.write(&mut p);
    let reply = ctx.invoke(owner, installer::GET_STAGED_SESSIONS, p)?;
    let mut r = reply.reader();
    r.read_exception().map_err(wire)??;
    if r.read_i32().map_err(wire)? == 0 {
        return Err(wire(BAD_VALUE));
    }
    let count = r.read_i32().map_err(wire)?;
    if count < 0 {
        return Err(wire(BAD_VALUE));
    }
    let mut items = Vec::new();
    if count > 0 {
        if r.read_string16().map_err(wire)?.as_deref()
            != Some("android.content.pm.PackageInstaller$SessionInfo")
        {
            return Err(wire(BAD_VALUE));
        }
    }
    while items.len() < count as usize {
        if r.read_i32().map_err(wire)? == 0 {
            break;
        }
        items.push(SessionInfo::read_from(&mut r).map_err(wire)?);
    }
    let retriever = if items.len() < count as usize {
        Some(
            r.read_binder()
                .map_err(wire)?
                .ok_or_else(|| wire(BAD_VALUE))?,
        )
    } else {
        None
    };
    if r.remaining() != 0 {
        return Err(wire(BAD_VALUE));
    }
    while items.len() < count as usize {
        let before = items.len();
        let mut request = Parcel::new();
        request.write_i32(before as i32);
        let reply = ctx.invoke(retriever.unwrap(), 1, request)?;
        let mut r = reply.reader();
        r.read_exception().map_err(wire)??;
        while items.len() < count as usize {
            if r.read_i32().map_err(wire)? == 0 {
                break;
            }
            items.push(SessionInfo::read_from(&mut r).map_err(wire)?);
        }
        if items.len() == before || r.remaining() != 0 {
            return Err(wire(BAD_VALUE));
        }
    }
    for s in &items {
        if s.parent_session_id != -1 || only_ready && !s.session_ready {
            continue;
        }
        print_session(ctx, s, only_id, "");
        if s.multi_package && !only_parent {
            for id in s.child_session_ids.as_deref().unwrap_or_default() {
                if let Some(child) = items.iter().find(|s| s.session_id == *id) {
                    print_session(ctx, child, only_id, "  ");
                } else if only_id {
                    ctx.command.println(&format!("  {id}"));
                } else {
                    ctx.command
                        .println(&format!("  sessionId = {id}; not found"));
                }
            }
        }
    }
    Ok(1)
}
fn print_session(ctx: &mut Context<'_>, s: &SessionInfo, id: bool, indent: &str) {
    if id {
        ctx.command.println(&format!("{indent}{}", s.session_id));
    } else {
        ctx.command.println(&format!("{indent}sessionId = {}; appPackageName = {}; isStaged = {}; isReady = {}; isApplied = {}; isFailed = {}; errorMsg = {};",s.session_id,s.app_package_name.as_deref().unwrap_or("null"),s.staged,s.session_ready,s.session_applied,s.session_failed,s.session_error_message.as_deref().unwrap_or("null")));
    }
}
struct Versioned {
    package: String,
    version: i64,
}
impl WriteParcelable for Versioned {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.package));
        p.write_i64(self.version);
    }
}
fn uninstall(ctx: &mut Context<'_>, owner: Binder, args: &[String]) -> Result<i32, Exception> {
    if !ctx.boot_completed()? {
        ctx.command.println("Error: device is still booting.");
        return Ok(1);
    }
    let (mut at, mut flags, mut requested, mut version) = (0, 0, -1, -1i64);
    while args.get(at).is_some_and(|a| a.starts_with('-')) {
        let opt = arg(args, &mut at)?;
        match opt.as_str() {
            "-k" => flags |= 1,
            "--user" => {
                requested = user(&arg(args, &mut at)?)?;
                if requested >= 0 && !ctx.user_exists(requested)? {
                    ctx.command
                        .println(&format!("Failure [user {requested} doesn't exist]"));
                    return Ok(1);
                }
            }
            "--versionCode" => version = integer(&arg(args, &mut at)?)?,
            _ => {
                ctx.command
                    .println(&format!("Error: Unknown option: {opt}"));
                return Ok(1);
            }
        }
    }
    let Some(package) = args.get(at).cloned() else {
        ctx.command.println("Error: package name not specified");
        return Ok(1);
    };
    at += 1;
    if at < args.len() {
        let mut p = params(ctx, &[])?;
        p.session = SessionParams {
            mode: 2,
            install_flags: 2,
            app_package_name: Some(package),
            size_bytes: -1,
            originating_uid: -1,
            required_installed_version_code: -1,
            unarchive_id: -1,
            auto_install_dependencies_enabled: true,
            ..Default::default()
        };
        let id = create(ctx, owner, &mut p)?;
        let result = (|| {
            remove(ctx, owner, id, &args[at..], false)?;
            commit(ctx, owner, id, 0)
        })();
        if !matches!(result, Ok(0)) {
            if let Err(error) = abandon(ctx, owner, id, false) {
                ctx.command
                    .eprintln(&format!("Failed to abandon session: {}", error.message));
            }
        }
        return result;
    }
    if requested == -1 {
        flags |= 2;
    }
    let translated = ctx.translate_user(requested, 0, "runUninstall")?;
    let package_flags = ctx.uninstall_package_flags(&package, translated)?;
    if flags & 2 == 0 && package_flags.is_none() {
        ctx.command
            .println(&format!("Failure [not installed for {translated}]"));
        return Ok(1);
    }
    if flags & 2 == 0 && package_flags.is_some_and(|(system, _)| system) {
        flags |= 4;
    }
    let state = Arc::new(ResultState::default());
    let binder = ctx.publish_receiver(Arc::new(Receiver(state.clone())))?;
    let receiver = IntentSender { target: binder };
    let result = (|| {
        if package_flags.is_some_and(|(_, apex)| apex) {
            ctx.uninstall_apex(&package, version, translated, receiver, flags)?;
        } else {
            let mut p = Parcel::new();
            installer::Uninstall {
                versioned_package: Some(Versioned { package, version }),
                caller_package_name: None,
                flags,
                status_receiver: Some(receiver),
                user_id: translated,
            }
            .write(&mut p);
            checked(ctx.invoke(owner, installer::UNINSTALL, p)?)?;
        }
        Ok(report(ctx, state.wait()?))
    })();
    ctx.retire_receiver(binder);
    result
}
/// `None` leaves unrelated commands to the remaining native shell cohorts.
pub fn run(ctx: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    let command = ctx.command.command().unwrap_or_default().to_owned();
    if !matches!(
        command.as_str(),
        "install"
            | "install-create"
            | "install-write"
            | "install-commit"
            | "install-abandon"
            | "install-add-session"
            | "install-remove"
            | "get-stagedsessions"
            | "remove-splits"
            | "uninstall"
    ) {
        return Ok(None);
    }
    let args = ctx.command.args[1..].to_vec();
    let owner = installer(ctx)?;
    let result = match command.as_str() {
        "install-create" => {
            let mut p = params(ctx, &args)?;
            if !p.paths.is_empty() {
                return Err(bad("Unexpected argument"));
            }
            let id = create(ctx, owner, &mut p)?;
            ctx.command
                .println(&format!("Success: created install session [{id}]"));
            0
        }
        "install-write" => {
            let mut at = 0;
            let mut size = -1;
            while args.get(at).is_some_and(|a| a.starts_with('-')) {
                let opt = arg(&args, &mut at)?;
                if opt != "-S" {
                    return Err(bad(format!("Unknown option: {opt}")));
                }
                size = integer(&arg(&args, &mut at)?)?;
            }
            let id = integer(&arg(&args, &mut at)?)?;
            let name = arg(&args, &mut at)?;
            let path = args.get(at).map(String::as_str);
            write(ctx, owner, id, name, path, size, true)?
        }
        "install-commit" => {
            let mut at = 0;
            let mut timeout = 60000;
            while args.get(at).is_some_and(|a| a.starts_with('-')) {
                let opt = arg(&args, &mut at)?;
                if opt != "--staged-ready-timeout" {
                    return Err(bad(format!("Unknown option: {opt}")));
                }
                timeout = integer(&arg(&args, &mut at)?)?;
            }
            commit(ctx, owner, integer(&arg(&args, &mut at)?)?, timeout)?
        }
        "install-abandon" => abandon(
            ctx,
            owner,
            integer(args.first().ok_or_else(|| bad("Missing session ID"))?)?,
            true,
        )?,
        "install-add-session" => {
            let mut at = 0;
            let parent = integer(&arg(&args, &mut at)?)?;
            if at == args.len() {
                ctx.command
                    .println("Error: At least two sessions are required.");
                1
            } else {
                with_session(ctx, owner, parent, |ctx, target| {
                    let mut p = Parcel::new();
                    session::IsMultiPackage {}.write(&mut p);
                    let reply = ctx.invoke(target, session::IS_MULTI_PACKAGE, p)?;
                    if !session::read_is_multi_package_reply(&mut reply.reader())
                        .map_err(wire)??
                    {
                        ctx.command
                            .eprintln("Error: parent session ID is not a multi-package session");
                        return Ok(1);
                    }
                    for child in &args[at..] {
                        let mut p = Parcel::new();
                        session::AddChildSessionId {
                            session_id: integer(child)?,
                        }
                        .write(&mut p);
                        checked(ctx.invoke(target, session::ADD_CHILD_SESSION_ID, p)?)?;
                    }
                    ctx.command.println("Success");
                    Ok(0)
                })?
            }
        }
        "install-remove" => {
            let mut at = 0;
            let id = integer(&arg(&args, &mut at)?)?;
            if at == args.len() {
                ctx.command.println("Error: split name not specified");
                1
            } else {
                remove(ctx, owner, id, &args[at..], true)?
            }
        }
        "get-stagedsessions" => staged_list(ctx, owner, &args)?,
        "uninstall" => uninstall(ctx, owner, &args)?,
        "remove-splits" => {
            let package = args.first().ok_or_else(|| bad("Missing package"))?;
            if args.len() < 2 {
                return Err(bad("Missing split name"));
            }
            let mut p = params(ctx, &[])?;
            p.session.mode = 2;
            p.session.app_package_name = Some(package.clone());
            let id = create(ctx, owner, &mut p)?;
            let result = (|| {
                remove(ctx, owner, id, &args[1..], false)?;
                commit(ctx, owner, id, 0)
            })();
            if !matches!(result, Ok(0)) {
                if let Err(error) = abandon(ctx, owner, id, false) {
                    ctx.command
                        .eprintln(&format!("Failed to abandon session: {}", error.message));
                }
            }
            result?
        }
        "install" => {
            let mut p = params(ctx, &args)?;
            if p.session.multi_package {
                return Err(bad("Unsupported option --multi-package"));
            }
            if !ctx.boot_completed()? {
                ctx.command.println("Error: device is still booting.");
                return Ok(Some(1));
            }
            if p.user >= 0 && !ctx.user_exists(p.user)? {
                ctx.command
                    .println(&format!("Failure [user {} doesn't exist]", p.user));
                return Ok(Some(1));
            }
            let stdin = p.paths.is_empty() || p.paths[0] == "-";
            if stdin && p.session.size_bytes == -1 {
                ctx.command
                    .println("Error: must either specify a package size or an APK file");
                return Ok(Some(1));
            }
            if stdin && p.paths.len() > 1 {
                ctx.command
                    .println("Error: can't specify SPLIT(s) along with STDIN");
                return Ok(Some(1));
            }
            if p.session.install_flags & 0x20000 != 0 && p.paths.len() > 1 {
                ctx.command
                    .println("Error: can't specify SPLIT(s) for APEX");
                return Ok(Some(1));
            }
            if p.paths.is_empty() {
                p.paths.push("-".into());
            }
            if !stdin && p.session.size_bytes == -1 {
                let mut size = 0i64;
                for path in &p.paths {
                    size = size
                        .checked_add(ctx.install_size(path, p.session.abi_override.as_deref())?)
                        .ok_or_else(|| bad("APK size overflow"))?;
                }
                p.session.size_bytes = size;
            }
            let id = create(ctx, owner, &mut p)?;
            let result = (|| {
                for path in &p.paths {
                    let name = if p.paths.len() == 1 {
                        if p.session.install_flags & 0x20000 != 0 {
                            "base.apex".to_owned()
                        } else {
                            "base.apk".to_owned()
                        }
                    } else {
                        std::path::Path::new(path)
                            .file_name()
                            .and_then(|v| v.to_str())
                            .ok_or_else(|| bad("Invalid APK filename"))?
                            .to_owned()
                    };
                    if write(
                        ctx,
                        owner,
                        id,
                        name,
                        Some(path),
                        p.session.size_bytes,
                        false,
                    )? != 0
                    {
                        return Ok(1);
                    }
                }
                commit(ctx, owner, id, p.timeout)
            })();
            if !matches!(result, Ok(0)) {
                if let Err(error) = abandon(ctx, owner, id, false) {
                    ctx.command
                        .eprintln(&format!("Failed to abandon session: {}", error.message));
                }
            }
            result?
        }
        _ => unreachable!(),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod retained_install_file_tests {
    use super::*;
    use aim_binder_host::{local::LocalProcess, server, wire::RegularMetadata};
    use std::{fs::OpenOptions, io::{Read, Seek, SeekFrom, Write}, os::fd::AsRawFd, sync::Weak};
    struct Incoming(u32);
    impl ReadParcelable for Incoming {
        fn read_from(reader: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
            if reader.read_i32()? != 0 { return Err(BAD_VALUE); }
            Ok(Self(reader.read_fd()?))
        }
    }
    struct SessionLeaf(Weak<LocalProcess>);
    impl Service for SessionLeaf {
        fn descriptor(&self) -> &str { session::DESCRIPTOR }
        fn accepts_fds(&self) -> bool { true }
        fn transact(&self, call: &mut Call<'_>) -> aim_binder_host::local::Reply {
            assert_eq!(call.code, session::WRITE);
            let input = session::Write::<Incoming>::read(&mut call.data)?;
            assert_eq!(call.data.remaining(), 0);
            let process = self.0.upgrade().unwrap();
            let file = process.file(input.fd.unwrap().0).unwrap();
            assert_eq!(server::file_class(&file), Some(aim_binder_host::regular_file::CLASS));
            let retained = server::file_fd(&file).unwrap();
            // Production sizes the actual APK before consuming its retained owner.
            assert_eq!(retained.metadata().unwrap().len() as i64, input.length_bytes);
            let mut reply = Parcel::new();
            reply.write_file(retained.into_file_owner());
            Ok(reply)
        }
    }
    #[test]
    fn real_install_leaf_reexport_retains_regular_identity_and_writer() {
        let root=std::env::temp_dir().join(format!("aim-install-retained-file-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let mut source=OpenOptions::new().read(true).write(true).create_new(true).open(root.join("apk")).unwrap();
        source.write_all(b"real transfer payload").unwrap();
        let writer=OpenOptions::new().read(true).write(true).create_new(true).open(root.join("writer")).unwrap();
        let contender=OpenOptions::new().read(true).write(true).open(root.join("writer")).unwrap();
        assert_eq!(unsafe {libc::flock(writer.as_raw_fd(),libc::LOCK_SH)},0);
        let metadata=RegularMetadata { flags:2,uid:2000,gid:2000,
            identity:aim_binder_host::regular_file::identity(source.as_fd()).unwrap(), writer:true };
        let original=server::regular_file_from_fd(source.as_fd(),Some(writer.as_fd()),metadata).unwrap();
        let retained=server::file_fd(&original).unwrap();
        assert_eq!(retained.metadata().unwrap().len(), b"real transfer payload".len() as u64);
        let downgraded=server::file_from_fd(retained.as_fd()).unwrap();
        assert_ne!(server::file_class(&downgraded),Some(aim_binder_host::regular_file::CLASS));
        drop(downgraded);
        let mut request=Parcel::new();
        session::Write { name:Some("base.apk".into()),offset_bytes:0,length_bytes:b"real transfer payload".len() as i64,
            fd:Some(Fd(retained.into_file_owner())) }.write(&mut request);
        let driver=aim_binder_driver::Driver::new();
        let process=LocalProcess::open(&driver,aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials{pid:194123,euid:1000,security_context:None});
        let Binder::Local(ptr)=process.add_service(Arc::new(SessionLeaf(Arc::downgrade(&process)))) else {panic!("local session")};
        let reply=process.local_service(ptr).unwrap().transact(session::WRITE,&request,false).unwrap();
        let file=process.file(reply.reader().read_fd().unwrap()).unwrap();
        assert_eq!(server::file_class(&file),Some(aim_binder_host::regular_file::CLASS));
        // The exact allocation capability carries its metadata and writer receipt.
        assert!(Arc::ptr_eq(&file,&original));
        let mut held=server::file_fd(&file).unwrap();
        assert_eq!(aim_binder_host::regular_file::identity(held.as_fd()).unwrap(),aim_binder_host::regular_file::identity(source.as_fd()).unwrap());
        drop(reply);drop(request);drop(original);drop(source);drop(writer);drop(file);
        held.seek(SeekFrom::Start(0)).unwrap();let mut actual=Vec::new();held.read_to_end(&mut actual).unwrap();
        assert_eq!(actual,b"real transfer payload");
        assert_eq!(unsafe {libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},-1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::EWOULDBLOCK));
        drop(held);
        assert_eq!(unsafe {libc::flock(contender.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)},0);
        drop(contender);
        std::fs::remove_dir_all(root).unwrap();
    }
}

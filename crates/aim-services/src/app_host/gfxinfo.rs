//! AMS GraphicsBinder, dumpGraphicsHardwareUsage and ProcessList.collectProcessesLOSP
//! at android-16.0.0_r1, with DumpUtils' permission policy and TransferPipe's
//! per-process EOF/5-second boundary. HWUI owns the body and its reset/framestats
//! arguments; the native process owner supplies names, pids and recency.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, DUMP_TRANSACTION, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Exception, FAILED_TRANSACTION, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_app_iapplicationthread as application;

use super::{AppHost, OP_GET_USAGE_STATS, PACKAGE_USAGE_STATS, appops_service, clock_ms};
use crate::package::app_metadata::Descriptor;

pub(super) struct GraphicsBinder(pub Arc<AppHost>);

impl GraphicsBinder {
    // DumpUtils.checkDumpAndUsageStatsPermission: usage permission is required
    // even when the app-op is allowed; system-internal UIDs bypass only usage.
    fn denial(&self, call: &Call<'_>) -> Result<Option<String>, Exception> {
        let uid = call.sender_euid as i32;
        let reason = if self
            .0
            .check_permission(Some("android.permission.DUMP"), uid)?
            != 0
        {
            Some("missing android.permission.DUMP permission")
        } else if matches!(uid, 0 | 1000 | 2000 | 1067) {
            None
        } else if self.0.check_permission(Some(PACKAGE_USAGE_STATS), uid)? != 0 {
            Some("missing android.permission.PACKAGE_USAGE_STATS permission")
        } else {
            let (state, resolution) = self.0.packages()?;
            let query = crate::package::query::Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: 1000,
            };
            let packages = query
                .packages_for_uid(uid)
                .map_err(|e| self.0.unimplemented_named("gfxinfo", e.0))?;
            let mut allowed = false;
            for package in packages.into_iter().flatten().flatten() {
                let mode = appops_service::note(&self.0, OP_GET_USAGE_STATS, uid, Some(&package))?;
                if matches!(
                    mode,
                    appops_service::MODE_ALLOWED | appops_service::MODE_DEFAULT
                ) {
                    allowed = true;
                    break;
                }
            }
            (!allowed).then_some("android:get_usage_stats app-op not allowed")
        };
        Ok(reason.map(|reason| format!(
            "Permission Denial: can't dump gfxinfo from from pid={}, uid={uid} due to {reason}\n",
            call.sender_pid
        )))
    }

    fn dump(&self, out: &mut impl Write, args: Option<Vec<Option<String>>>) -> io::Result<()> {
        let selector = args
            .as_ref()
            .and_then(|a| a.first())
            .and_then(|a| a.as_deref())
            .filter(|a| !a.starts_with('-'));
        let pid = selector.and_then(|s| s.parse::<i32>().ok());
        // The host keeps process activity recency rather than a Java ProcessList.
        // A filtered collect walks newest first, then AMS dumps it in reverse;
        // an unfiltered collect copies the LRU and AMS dumps newest first.
        let mut processes: Vec<_> = self
            .0
            .apps
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, app)| {
                selector
                    .is_none_or(|name| (app.pid > 0 && Some(app.pid) == pid) || app.process == name)
            })
            .map(|(seq, app)| {
                (
                    (app.last_activity.max(app.start_uptime), *seq),
                    app.pid,
                    app.process.clone(),
                    app.thread.as_ref().map(|t| self.0.process.strong(t.handle)),
                )
            })
            .collect();
        processes.sort_by_key(|p| p.0);
        if selector.is_none() {
            processes.reverse();
        }
        if let Some(selector) = selector
            && processes.is_empty()
        {
            return writeln!(out, "No process found for: {selector}");
        }
        writeln!(out, "Applications Graphics Acceleration Info:")?;
        writeln!(
            out,
            "Uptime: {} Realtime: {}",
            clock_ms(libc::CLOCK_UPTIME_RAW),
            clock_ms(libc::CLOCK_MONOTONIC_RAW)
        )?;
        for (_, pid, name, thread) in processes {
            let Some(thread) = thread else { continue };
            writeln!(out, "\n** Graphics info for pid {pid} [{name}] **")?;
            out.flush()?;
            match dump_process(&thread, &args, out, Duration::from_secs(5)) {
                Ok(()) => {}
                Err(DumpError::Io(error)) => {
                    eprintln!("app host: gfxinfo {pid} [{name}]: {error}");
                    writeln!(out, "Failure while dumping the app: {pid}:{name}")?;
                }
                Err(DumpError::Remote(status)) => {
                    eprintln!("app host: gfxinfo {pid} [{name}]: binder status {status}");
                    writeln!(
                        out,
                        "Got a RemoteException while dumping the app {pid}:{name}"
                    )?;
                }
            }
            out.flush()?;
        }
        Ok(())
    }
}

impl Service for GraphicsBinder {
    fn descriptor(&self) -> &str {
        ""
    }
    fn has_descriptor(&self) -> bool {
        false
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != DUMP_TRANSACTION {
            return Err(UNKNOWN_TRANSACTION);
        }
        let fd = call.data.read_fd()?;
        let mut out = self
            .0
            .process
            .file(fd)
            .and_then(|file| aim_binder_host::server::file_fd(&file))
            .ok_or(BAD_VALUE)?;
        let args = aim_service_aidl::read_string_list(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        match self.denial(call) {
            Ok(Some(denial)) => out
                .write_all(denial.as_bytes())
                .map_err(|_| FAILED_TRANSACTION)?,
            Ok(None) => self.dump(&mut out, args).map_err(|_| FAILED_TRANSACTION)?,
            Err(e) => return super::reply(Err(e)),
        }
        let mut reply = Parcel::new();
        reply.write_no_exception();
        Ok(reply)
    }
}

enum DumpError {
    Io(io::Error),
    Remote(i32),
}
impl From<io::Error> for DumpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn dump_process(
    thread: &Strong,
    args: &Option<Vec<Option<String>>>,
    out: &mut impl Write,
    timeout: Duration,
) -> Result<(), DumpError> {
    let mut fds = [-1; 2];
    // SAFETY: pipe initializes both owned descriptors on success.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let (mut read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    let file = aim_binder_host::server::file_from_fd(write.as_fd())
        .ok_or_else(|| io::Error::other("gfxinfo pipe export failed"))?;
    let mut request = Parcel::new();
    // IApplicationThread.dumpGfxInfo(in ParcelFileDescriptor fd, in String[] args).
    request.write_interface_token(application::DESCRIPTOR);
    aim_service_aidl::write_typed(&mut request, Some(&Descriptor(file)));
    aim_service_aidl::write_string_list(&mut request, args.as_deref());
    thread
        .transact(application::DUMP_GFX_INFO, &request, true)
        .map_err(DumpError::Remote)?;
    drop(request);
    drop(write);
    drain(&mut read, out, timeout)?;
    Ok(())
}

fn drain(read: &mut File, out: &mut impl Write, timeout: Duration) -> io::Result<()> {
    let end = Instant::now() + timeout;
    let mut buffer = [0; 4096];
    loop {
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Timeout"));
        }
        let mut fd = libc::pollfd {
            fd: read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let result = unsafe {
            libc::poll(
                &mut fd,
                1,
                remaining.as_millis().max(1).min(i32::MAX as u128) as i32,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if result == 0 {
            continue;
        }
        match read.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => out.write_all(&buffer[..count])?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver, uapi::*};
    use aim_binder_host::local::LocalProcess;
    use aim_binder_host::parcel::Binder;
    use std::sync::{Mutex, mpsc};

    struct NoMemory;
    impl aim_binder_driver::GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), aim_binder_driver::Errno> {
            Err(aim_binder_driver::errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), aim_binder_driver::Errno> {
            Err(aim_binder_driver::errno::EFAULT)
        }
        fn get_file(
            &mut self,
            _: u32,
        ) -> Result<aim_binder_driver::File, aim_binder_driver::Errno> {
            Err(aim_binder_driver::errno::EBADF)
        }
        fn install_file(
            &mut self,
            _: aim_binder_driver::File,
        ) -> Result<u32, aim_binder_driver::Errno> {
            Err(aim_binder_driver::errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            unreachable!()
        }
    }

    struct ApplicationThread {
        process: Arc<LocalProcess>,
        calls: mpsc::Sender<Option<Vec<Option<String>>>>,
        body: Vec<u8>,
        hold: Mutex<Option<mpsc::Sender<aim_binder_host::server::RetainedFd>>>,
    }
    impl Service for ApplicationThread {
        fn descriptor(&self) -> &str {
            application::DESCRIPTOR
        }
        fn accepts_fds(&self) -> bool {
            true
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, application::DUMP_GFX_INFO);
            assert!(call.is_oneway());
            assert_eq!(call.sender_euid, 1000);
            call.data.enforce_interface(application::DESCRIPTOR)?;
            assert_eq!(call.data.read_i32()?, 1); // typed ParcelFileDescriptor
            assert_eq!(call.data.read_i32()?, 0); // no comm fd
            let file = self.process.file(call.data.read_fd()?).unwrap();
            let mut fd = aim_binder_host::server::file_fd(&file).unwrap();
            let args = aim_service_aidl::read_string_list(&mut call.data)?;
            assert_eq!(call.data.remaining(), 0);
            self.calls.send(args).unwrap();
            if let Some(hold) = self.hold.lock().unwrap().as_ref() {
                hold.send(fd).unwrap();
            } else {
                fd.write_all(&self.body).unwrap();
            }
            Ok(Parcel::new())
        }
    }

    struct Fixture {
        host: Arc<AppHost>,
        app: Arc<LocalProcess>,
        calls: mpsc::Receiver<Option<Vec<Option<String>>>>,
        thread: Arc<ApplicationThread>,
    }
    impl Fixture {
        fn new(body: &[u8]) -> Self {
            let driver = Driver::new();
            let open = |pid| {
                LocalProcess::open(
                    &driver,
                    Device::Binder,
                    Credentials {
                        pid,
                        euid: 1000,
                        security_context: None,
                    },
                )
            };
            let native = open(97101);
            let app = open(97102);
            let host = AppHost::new(native);
            let permissions = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            let (calls, received) = mpsc::channel();
            let thread = Arc::new(ApplicationThread {
                process: app.clone(),
                calls,
                body: body.into(),
                hold: Mutex::new(None),
            });
            let Binder::Local(pointer) = app.add_service(thread.clone()) else {
                unreachable!()
            };
            let mut object = FlatBinderObject {
                kind: BINDER_TYPE_BINDER,
                flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
                binder: pointer,
                cookie: pointer,
            }
            .encode();
            driver
                .ioctl(
                    app.proc_handle(),
                    97103,
                    BINDER_SET_CONTEXT_MGR_EXT,
                    &mut object,
                    &mut NoMemory,
                )
                .unwrap();
            app.start();
            Self {
                host,
                app,
                calls: received,
                thread,
            }
        }
        fn add(&self, seq: i64, pid: i32, name: &str, recency: i64, attached: bool) {
            let template = super::super::tests::host_with_start(seq, Arc::new(|| {}));
            let mut app = template.apps.lock().unwrap().remove(&seq).unwrap();
            app.process = name.into();
            app.package = "primary.package".into();
            app.packages = vec!["primary.package".into(), "shared.package".into()];
            app.pid = pid;
            app.last_activity = recency;
            app.thread = attached.then(|| self.host.process.strong(0));
            self.host.apps.lock().unwrap().insert(seq, app);
        }
        fn dump(&self, args: &[&str]) -> String {
            let mut output = Vec::new();
            GraphicsBinder(self.host.clone())
                .dump(
                    &mut output,
                    Some(args.iter().map(|s| Some((*s).into())).collect()),
                )
                .unwrap();
            String::from_utf8(output).unwrap()
        }
    }
    fn dump_service(host: &Arc<AppHost>, args: &[&str], uid: u32) -> String {
        let directory = super::super::test_temp_dir("gfxinfo");
        let path = directory.join("output");
        let output = File::create(&path).unwrap();
        let mut request = Parcel::new();
        request.write_file(aim_binder_host::server::file_from_fd(output.as_fd()).unwrap());
        aim_service_aidl::write_string_list(
            &mut request,
            Some(&args.iter().map(|s| Some((*s).into())).collect::<Vec<_>>()),
        );
        let service = Arc::new(GraphicsBinder(host.clone()));
        let Binder::Local(pointer) = host.process.add_service(service.clone()) else {
            unreachable!()
        };
        let node = host.process.local_service(pointer).unwrap();
        let response = if uid == 1000 {
            node.transact(DUMP_TRANSACTION, &request, false)
                .unwrap()
                .into_parcel()
        } else {
            // Install real fileports before calling with the denied caller's identity.
            struct Output(aim_binder_driver::File);
            impl Service for Output {
                fn descriptor(&self) -> &str {
                    ""
                }
                fn transact(&self, _: &mut Call<'_>) -> Reply {
                    let mut p = Parcel::new();
                    p.write_file(self.0.clone());
                    Ok(p)
                }
            }
            let Binder::Local(pointer) = host.process.add_service(Arc::new(Output(
                aim_binder_host::server::file_from_fd(output.as_fd()).unwrap(),
            ))) else {
                unreachable!()
            };
            let received = host
                .process
                .local_service(pointer)
                .unwrap()
                .transact(1, &Parcel::new(), false)
                .unwrap();
            let mut p = received.into_parcel();
            aim_service_aidl::write_string_list(
                &mut p,
                Some(&args.iter().map(|s| Some((*s).into())).collect::<Vec<_>>()),
            );
            service
                .transact(&mut Call {
                    code: DUMP_TRANSACTION,
                    flags: 0,
                    sender_pid: 42,
                    sender_euid: uid,
                    data: p.reader(),
                })
                .unwrap()
        };
        response.reader().read_exception().unwrap().unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
        text
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.host.process.shutdown().unwrap();
            self.app.shutdown().unwrap();
        }
    }

    #[test]
    fn gfxinfo_passes_pid_process_reset_and_framestats_to_hwui_over_binder() {
        let fixture = Fixture::new(b"---PROFILEDATA---\nHWUI frame data\n---PROFILEDATA---\n");
        fixture.add(7, 4321, "example.app", 10, true);
        fixture.add(8, 4322, "example.app:worker", 20, true);
        for args in [
            vec!["example.app", "reset"],
            vec!["4321", "framestats"],
            vec!["example.app:worker", "framestats", "reset"],
        ] {
            let text = fixture.dump(&args);
            let sent = fixture.calls.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(sent, Some(args.iter().map(|s| Some((*s).into())).collect()));
            let (pid, name) = if args[0].contains(":worker") {
                (4322, "example.app:worker")
            } else {
                (4321, "example.app")
            };
            assert!(text.starts_with("Applications Graphics Acceleration Info:\nUptime: "));
            assert!(text.contains(" Realtime: "));
            assert!(text.ends_with(&format!("\n** Graphics info for pid {pid} [{name}] **\n---PROFILEDATA---\nHWUI frame data\n---PROFILEDATA---\n")));
            assert_eq!(text.matches("** Graphics info").count(), 1);
        }
    }

    #[test]
    fn gfxinfo_orders_headers_and_large_async_bodies_and_skips_unattached_processes() {
        let body = vec![b'x'; 128 * 1024];
        let fixture = Fixture::new(&body);
        fixture.add(7, 4321, "older", 10, true);
        fixture.add(8, 4322, "newer", 20, true);
        fixture.add(9, 4323, "not-attached", 30, false);
        let text = fixture.dump(&[]);
        let first = text
            .find("** Graphics info for pid 4322 [newer] **\n")
            .unwrap();
        let second = text
            .find("** Graphics info for pid 4321 [older] **\n")
            .unwrap();
        assert!(first < second);
        assert_eq!(text.matches('x').count(), body.len() * 2);
        assert!(!text.contains("not-attached"));
        assert!(text.ends_with(std::str::from_utf8(&body).unwrap()));
        for _ in 0..2 {
            assert_eq!(
                fixture.calls.recv_timeout(Duration::from_secs(1)).unwrap(),
                Some(vec![])
            );
        }
    }

    #[test]
    fn gfxinfo_unknown_package_is_reported_without_asking_any_application() {
        let fixture = Fixture::new(b"unreachable");
        fixture.add(7, 4321, "example.app:worker", 10, true);
        // allPkgs=false in the original: hosted package names are not selectors.
        for name in ["unknown.package", "shared.package", "example.app", "99999"] {
            assert_eq!(
                fixture.dump(&[name]),
                format!("No process found for: {name}\n")
            );
        }
        assert!(fixture.calls.try_recv().is_err());
    }

    #[test]
    fn gfxinfo_denies_dump_before_selecting_or_contacting_processes() {
        crate::system::with_discovery_binders(|host, _, _, _| {
            let permissions = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            assert_eq!(
                dump_service(host, &["example.app", "reset"], 99001),
                "Permission Denial: can't dump gfxinfo from from pid=42, uid=99001 due to missing android.permission.DUMP permission\n"
            );
            assert_eq!(
                dump_service(host, &["unknown.package"], 1000),
                "No process found for: unknown.package\n"
            );
            assert_eq!(
                dump_service(host, &[], 1000)
                    .matches("** Graphics info")
                    .count(),
                0
            );
        });
    }

    #[test]
    fn gfxinfo_dash_options_select_all_and_filtered_process_order_is_reversed() {
        let fixture = Fixture::new(b"HWUI\n");
        fixture.add(7, 4321, "shared.process", 10, true);
        fixture.add(8, 4322, "shared.process", 20, true);
        for (args, first, second) in [
            (vec!["-a", "framestats"], 4322, 4321),
            (vec!["shared.process", "framestats"], 4321, 4322),
        ] {
            let text = fixture.dump(&args);
            assert!(
                text.find(&format!("pid {first} ")).unwrap()
                    < text.find(&format!("pid {second} ")).unwrap()
            );
            assert_eq!(text.matches("HWUI\n").count(), 2);
            for _ in 0..2 {
                assert_eq!(
                    fixture.calls.recv_timeout(Duration::from_secs(1)).unwrap(),
                    Some(args.iter().map(|s| Some((*s).into())).collect())
                );
            }
        }
    }

    #[test]
    fn gfxinfo_transfer_timeout_releases_its_reader_and_does_not_wait_for_a_retained_writer() {
        let fixture = Fixture::new(b"");
        let (send, receive) = mpsc::channel();
        *fixture.thread.hold.lock().unwrap() = Some(send);
        let start = Instant::now();
        let result = dump_process(
            &fixture.host.process.strong(0),
            &Some(vec![]),
            &mut Vec::new(),
            Duration::from_millis(30),
        );
        assert!(matches!(result, Err(DumpError::Io(e)) if e.kind() == io::ErrorKind::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(receive.recv_timeout(Duration::from_secs(1)).unwrap());
    }
}

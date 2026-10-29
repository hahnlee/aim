//! Native kernel startup readiness, bound to the launched init's birth (#1265).
use aim_binder_host::{mach, wire};
use aim_storage::process_namespace::{InitRegistration, ProcessIdentity};
use std::path::Path;

pub(super) fn poll(
    runtime: &Path,
    init: ProcessIdentity,
    name: &str,
    timeout_ms: u32,
) -> Result<bool, String> {
    if !init.is_live() {
        return Err("template init exited before Binder readiness".into());
    }
    let table = runtime.join("identity/by-pid");
    let registration = match InitRegistration::read(&table) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("template init registration: {error}")),
    };
    if registration.process != init {
        return Err("template init registration belongs to another process birth".into());
    }
    let service = match mach::look_up(name) {
        Ok(value) => value,
        Err(1102) => return Ok(false), // BOOTSTRAP_UNKNOWN_SERVICE, before native startup.
        Err(error) => return Err(format!("template Binder lookup {name}: {error:#x}")),
    };
    let reply = match mach::new_port(false) {
        Ok(value) => value,
        Err(error) => {
            mach::release_send(service);
            return Err(format!("template Binder reply port: {error:#x}"));
        }
    };
    let request = mach::Msg {
        id: wire::FILES,
        ports: vec![],
        data: 0u32.to_le_bytes().to_vec(),
    };
    let result = mach::call_bounded(
        &mut mach::Buffer::default(),
        service,
        reply,
        &request,
        timeout_ms,
    );
    mach::destroy_receive(reply);
    mach::release_send(service);
    let response = match result {
        Ok(value) => value,
        Err(0x10004003 | 0x10000004) if init.is_live() => return Ok(false),
        Err(error) => return Err(format!("template Binder startup exchange: {error:#x}")),
    };
    check_reply(&table, init, &registration, response)
}

fn check_reply(
    table: &Path,
    init: ProcessIdentity,
    registration: &InitRegistration,
    response: mach::Received,
) -> Result<bool, String> {
    let checked = (|| {
        if response.pid != init.host_pid || !init.is_live() {
            return Err("template Binder readiness reply has another process birth".into());
        }
        let current = InitRegistration::read(table)
            .map_err(|error| format!("template init registration after Binder reply: {error}"))?;
        if current.process != init || current.mount_namespace != registration.mount_namespace {
            return Err("template init registration changed during Binder readiness".into());
        }
        if response.id != wire::REPLY || response.data.len() < 4 {
            return Err("template Binder startup reply is malformed".into());
        }
        let status = i32::from_le_bytes(response.data[..4].try_into().unwrap());
        if status != 0 {
            return Err(format!("template Binder startup status: {status}"));
        }
        if response.data.len() < 8 {
            return Err("template Binder shared-file reply is truncated".into());
        }
        let total = u32::from_le_bytes(response.data[4..8].try_into().unwrap()) as usize;
        if total < response.ports.len() || response.data.len() != 8 + 24 * response.ports.len() {
            return Err("template Binder shared-file reply is malformed".into());
        }
        Ok(true)
    })();
    for port in response.ports {
        mach::release_send(port);
    }
    checked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        process::{Child, Command},
        time::Duration,
    };
    struct Fixture {
        root: std::path::PathBuf,
        child: Child,
    }
    impl Fixture {
        fn new() -> Self {
            // This file is compiled into several test modules, and its tests
            // run at once.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "aim-binder-startup-{}-{}-{}",
                std::process::id(),
                module_path!().replace("::", "-"),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let child = Command::new("/bin/sleep").arg("20").spawn().unwrap();
            Self { root, child }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.child.try_wait().unwrap().is_none() {
                self.child.kill().unwrap();
            }
            self.child.wait().unwrap();
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    // Queue, serve and receive one request in order. The reply's audit
    // identity comes from the kernel, without a server scheduling deadline.
    fn kernel_reply(before_reply: impl FnOnce()) -> mach::Received {
        let service = mach::new_port(true).unwrap();
        let reply = mach::new_port(false).unwrap();
        let mut buffer = mach::Buffer::default();
        assert!(matches!(
            mach::call_bounded(
                &mut buffer,
                service,
                reply,
                &mach::Msg {
                    id: wire::FILES,
                    ports: vec![],
                    data: 0u32.to_le_bytes().to_vec(),
                },
                0,
            ),
            Err(0x10004003)
        ));
        let request = mach::receive(&mut buffer, service).unwrap();
        assert_eq!(request.id, wire::FILES);
        assert_eq!(request.data, 0u32.to_le_bytes());
        assert!(request.ports.is_empty());
        before_reply();
        mach::reply(
            &mut buffer,
            request.reply,
            &mach::Msg {
                id: wire::REPLY,
                ports: vec![],
                data: vec![0; 8],
            },
        );
        let result = mach::receive(&mut buffer, reply);
        mach::destroy_receive(reply);
        mach::release_send(service);
        mach::destroy_receive(service);
        result.unwrap()
    }
    #[test]
    fn actual_kernel_reply_is_required_and_init_birth_is_checked() {
        let mut fixture = Fixture::new();
        let child = ProcessIdentity::running(fixture.child.id() as i32).unwrap();
        fs::write(fixture.root.join("path-map"), b"early file only").unwrap();
        let name = format!(
            "dev.aim.template-startup.{}.{}",
            std::process::id(),
            child.host_pid
        );
        assert!(!poll(&fixture.root, child, &name, 20).unwrap());
        InitRegistration::register(&fixture.root.join("identity/by-pid"), child, "initial")
            .unwrap();
        let registration = InitRegistration::read(&fixture.root.join("identity/by-pid")).unwrap();
        assert!(
            check_reply(
                &fixture.root.join("identity/by-pid"),
                child,
                &registration,
                kernel_reply(|| {}),
            )
            .unwrap_err()
            .contains("another process birth")
        );
        let actual = ProcessIdentity::running(std::process::id() as i32).unwrap();
        let registration =
            InitRegistration::register(&fixture.root.join("identity/by-pid"), actual, "initial")
                .unwrap();
        assert!(
            check_reply(
                &fixture.root.join("identity/by-pid"),
                actual,
                &registration,
                kernel_reply(|| {}),
            )
            .unwrap()
        );
        registration
            .remove(&fixture.root.join("identity/by-pid"))
            .unwrap();
        // A boot that is retiring must not be admitted.
        assert!(!poll(&fixture.root, actual, &name, 20).unwrap());
        InitRegistration::register(&fixture.root.join("identity/by-pid"), child, "initial")
            .unwrap();
        assert!(
            poll(&fixture.root, actual, &name, 20)
                .unwrap_err()
                .contains("another process birth")
        );
        fixture.child.kill().unwrap();
        fixture.child.wait().unwrap();
        assert!(
            poll(&fixture.root, child, &name, 20)
                .unwrap_err()
                .contains("exited")
        );
    }
    #[test]
    fn registration_is_rechecked_after_the_kernel_reply() {
        let fixture = Fixture::new();
        let actual = ProcessIdentity::running(std::process::id() as i32).unwrap();
        let table = fixture.root.join("identity/by-pid");
        let registration = InitRegistration::register(&table, actual, "initial").unwrap();
        let response = kernel_reply(|| {
            InitRegistration::register(&table, actual, "replacement").unwrap();
        });
        assert!(
            check_reply(&table, actual, &registration, response)
                .unwrap_err()
                .contains("changed during Binder readiness")
        );
        let response = kernel_reply(|| {
            InitRegistration::read(&table)
                .unwrap()
                .remove(&table)
                .unwrap();
        });
        assert!(
            check_reply(&table, actual, &registration, response)
                .unwrap_err()
                .contains("registration after Binder reply")
        );
    }
    #[test]
    fn checked_in_port_without_serving_is_not_ready() {
        let fixture = Fixture::new();
        let actual = ProcessIdentity::running(std::process::id() as i32).unwrap();
        InitRegistration::register(&fixture.root.join("identity/by-pid"), actual, "initial")
            .unwrap();
        let name = format!(
            "dev.aim.template-not-serving.{}.{}",
            std::process::id(),
            fixture.child.id()
        );
        assert!(!poll(&fixture.root, actual, &name, 20).unwrap());
        let port = mach::check_in(&name).unwrap();
        let start = std::time::Instant::now();
        assert!(!poll(&fixture.root, actual, &name, 20).unwrap());
        assert!(start.elapsed() < Duration::from_secs(1));
        mach::destroy_receive(port);
    }
}

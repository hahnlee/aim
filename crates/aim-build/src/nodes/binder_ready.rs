//! Native kernel startup readiness, bound to the launched init's birth (#1265).
use aim_binder_host::{mach, wire};
use aim_storage::process_namespace::{InitRegistration, ProcessIdentity};
use std::path::Path;

pub(super) fn poll(runtime: &Path, init: ProcessIdentity, name: &str, timeout_ms: u32) -> Result<bool, String> {
    if !init.is_live() { return Err("template init exited before Binder readiness".into()); }
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
    let request = mach::Msg { id: wire::FILES, ports: vec![], data: 0u32.to_le_bytes().to_vec() };
    let result = mach::call_bounded(&mut mach::Buffer::default(), service, reply, &request, timeout_ms);
    mach::destroy_receive(reply);
    mach::release_send(service);
    let response = match result {
        Ok(value) => value,
        Err(0x10004003 | 0x10000004) if init.is_live() => return Ok(false),
        Err(error) => return Err(format!("template Binder startup exchange: {error:#x}")),
    };
    let checked = (|| {
        if response.pid != init.host_pid || !init.is_live() {
            return Err("template Binder readiness reply has another process birth".into());
        }
        let current = InitRegistration::read(&table)
            .map_err(|error| format!("template init registration after Binder reply: {error}"))?;
        if current.process != init || current.mount_namespace != registration.mount_namespace {
            return Err("template init registration changed during Binder readiness".into());
        }
        if response.id != wire::REPLY || response.data.len() < 4 {
            return Err("template Binder startup reply is malformed".into());
        }
        let status = i32::from_le_bytes(response.data[..4].try_into().unwrap());
        if status != 0 { return Err(format!("template Binder startup status: {status}")); }
        if response.data.len() < 8 {
            return Err("template Binder shared-file reply is truncated".into());
        }
        let total = u32::from_le_bytes(response.data[4..8].try_into().unwrap()) as usize;
        if total < response.ports.len() || response.data.len() != 8 + 24 * response.ports.len() {
            return Err("template Binder shared-file reply is malformed".into());
        }
        Ok(true)
    })();
    for port in response.ports { mach::release_send(port); }
    checked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::{Child, Command}, time::{Duration, SystemTime, UNIX_EPOCH}};
    struct Fixture { root: std::path::PathBuf, child: Child }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("aim-binder-startup-{}-{}", std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir(&root).unwrap();
            let child = Command::new("/bin/sleep").arg("20").spawn().unwrap();
            Self { root, child }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.child.try_wait().unwrap().is_none() { self.child.kill().unwrap(); }
            self.child.wait().unwrap();
            fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[test]
    fn actual_kernel_reply_is_required_and_init_birth_is_checked() {
        let mut fixture = Fixture::new();
        let child = ProcessIdentity::running(fixture.child.id() as i32).unwrap();
        fs::write(fixture.root.join("path-map"), b"early file only").unwrap();
        let name = format!("dev.aim.template-startup.{}.{}", std::process::id(), child.host_pid);
        assert!(!poll(&fixture.root, child, &name, 20).unwrap());
        InitRegistration::register(&fixture.root.join("identity/by-pid"), child, "initial").unwrap();
        assert!(!poll(&fixture.root, child, &name, 20).unwrap());
        let server = aim_binder_host::server::Server::start(&name).unwrap();
        assert!(poll(&fixture.root, child, &name, 20).unwrap_err().contains("another process birth"));
        let actual = ProcessIdentity::running(std::process::id() as i32).unwrap();
        InitRegistration::register(&fixture.root.join("identity/by-pid"), actual, "initial").unwrap();
        assert!(poll(&fixture.root, actual, &name, 20).unwrap());
        InitRegistration::register(&fixture.root.join("identity/by-pid"), child, "initial").unwrap();
        assert!(poll(&fixture.root, actual, &name, 20).unwrap_err().contains("another process birth"));
        fixture.child.kill().unwrap(); fixture.child.wait().unwrap();
        assert!(poll(&fixture.root, child, &name, 20).unwrap_err().contains("exited"));
        drop(server);
    }
    #[test]
    fn checked_in_port_without_serving_is_not_ready() {
        let fixture = Fixture::new();
        let actual = ProcessIdentity::running(std::process::id() as i32).unwrap();
        InitRegistration::register(&fixture.root.join("identity/by-pid"), actual, "initial").unwrap();
        let name = format!("dev.aim.template-not-serving.{}.{}", std::process::id(), fixture.child.id());
        let port = mach::check_in(&name).unwrap();
        let start = std::time::Instant::now();
        assert!(!poll(&fixture.root, actual, &name, 20).unwrap());
        assert!(start.elapsed() < Duration::from_secs(1));
        mach::destroy_receive(port);
    }
}

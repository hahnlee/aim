//! `linux-run --pidns-watch ... TABLE`: the members of a native init's pid
//! namespace die with it, even when it is SIGKILLed, as those of a Linux
//! pid namespace whose init exits (pid_namespaces(7)).

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aim_storage::process_namespace::ProcessIdentity;

struct Sleeper(Child);
impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn sleeper() -> Sleeper {
    Sleeper(Command::new("/bin/sleep").arg("60").spawn().unwrap())
}

#[test]
fn a_killed_init_takes_its_namespace_along() {
    let mut init = sleeper();
    let mut member = sleeper();
    let mut outside = sleeper();
    let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
    let table = std::env::temp_dir().join(format!("aim-pidns-watch-test-{}", init.0.id()));
    std::fs::create_dir(&table).unwrap();
    for process in [&init, &member] {
        std::fs::write(table.join(process.0.id().to_string()), "member").unwrap();
    }
    let mut watcher = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--pidns-watch")
        .arg(identity.host_pid.to_string())
        .arg(identity.start_seconds.to_string())
        .arg(identity.start_microseconds.to_string())
        .arg(&table)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(watcher.wait().unwrap().success());
    let mut ready = [0];
    watcher
        .stdout
        .take()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(ready, [1]);
    assert!(member.0.try_wait().unwrap().is_none());
    init.0.kill().unwrap();
    init.0.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = member.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "member outlived its init");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    assert!(outside.0.try_wait().unwrap().is_none());
    while table.exists() {
        assert!(Instant::now() < deadline, "table outlived its init");
        std::thread::sleep(Duration::from_millis(10));
    }
}

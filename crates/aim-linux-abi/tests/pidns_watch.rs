//! guest-init's watcher of its pid namespace (`linux-run --pidns-watch
//! ... TABLE`): the members of a native init's namespace die with it, even
//! when it is SIGKILLed, as those of a Linux pid namespace whose init exits
//! (pid_namespaces(7)).

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use aim_storage::process_namespace::ProcessIdentity;

/// Killing a sleeper that stands in for init also ends its watcher, so a
/// failed assertion leaves no watcher behind.
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

struct Dir(PathBuf);
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_killed_init_takes_its_namespace_along() {
    let mut member = sleeper();
    let mut outside = sleeper();
    let mut init = sleeper();
    let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
    let dir = Dir(std::env::temp_dir().join(format!("aim-pidns-watch-test-{}", init.0.id())));
    let table = dir.0.join("by-pid");
    std::fs::create_dir_all(&table).unwrap();
    for process in [&init, &member] {
        std::fs::write(table.join(process.0.id().to_string()), "member").unwrap();
    }
    let log = dir.0.join("pidns-watch.log");
    aim_guest_init::boot::watch_namespace(
        Path::new(env!("CARGO_BIN_EXE_linux-run")),
        &table,
        identity,
        &log,
    )
    .unwrap();
    assert!(member.0.try_wait().unwrap().is_none());
    init.0.kill().unwrap();
    init.0.wait().unwrap();
    // What the next boot does before it sweeps the runtime.
    aim_storage::process_namespace::wait_for_watcher(&table).unwrap();
    let mut status = None;
    wait_until("member outlived its init", || {
        status = member.0.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().signal(), Some(libc::SIGKILL));
    assert!(outside.0.try_wait().unwrap().is_none());
    assert!(table.exists(), "the table is the next boot's to sweep");
    assert_eq!(std::fs::read(&log).unwrap(), b"");
}

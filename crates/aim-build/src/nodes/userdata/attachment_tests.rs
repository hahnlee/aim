use super::*;
use aim_storage::disk;
use std::process::{Command, Stdio};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = aim_paths::root().join("target").join(format!(
            "userdata-attachment-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn work(&self) -> PathBuf {
        self.0.join("userdata-template.work")
    }

    fn assert_detached(&self) {
        for name in ["boot", "template"] {
            let dir = self.work().join(name);
            assert!(!disk::is_mount_point(&dir), "{} is mounted", dir.display());
            if data::image_of(&dir).exists() {
                assert!(
                    disk::attachments_of(&data::image_of(&dir))
                        .unwrap()
                        .is_empty()
                );
            }
        }
    }

    fn next_run(&self) {
        self.assert_detached();
        prepare_work(&self.work()).unwrap();
        assert!(!self.work().exists());
        let dir = self.work().join("boot");
        DataImage::attach(&dir, None).unwrap().detach().unwrap();
        self.assert_detached();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Also runs if an assertion fails, and only touches this fixture's images.
        for dir in [
            self.work().join("boot"),
            self.work().join("template"),
            self.0.join("other"),
        ] {
            if let Err(error) = data::remove(&dir) {
                eprintln!("fixture cleanup {}: {error}", dir.display());
                return;
            }
        }
        let _ = force_remove(&self.0);
    }
}

fn sleep(reference: &Path) -> std::process::Child {
    Command::new("/bin/sleep")
        .arg("60")
        // A real reference to the mounted volume must close before detach.
        .stdin(Stdio::from(fs::File::open(reference).unwrap()))
        .spawn()
        .unwrap()
}

fn session(owner: &mut TemplateBoot, pids: &mut Vec<u32>) {
    let mut image = DataImage::attach(&owner.dir, None).unwrap();
    let reference = image.dir().join("owned-reference");
    fs::write(&reference, "owned").unwrap();
    owner.display = Some(sleep(&reference));
    pids.push(owner.display.as_ref().unwrap().id());
    let init = sleep(&reference);
    pids.push(init.id());
    owner.session = Some(Session {
        display: owner.display.take().unwrap(),
        init,
    });
    image.handoff_detach().unwrap();
    // Simulate guest-init's handoff on exit: only the parent owns detach now.
}

fn host_session(owner: &mut TemplateBoot, pids: &mut Vec<u32>) {
    fs::create_dir_all(&owner.dir).unwrap();
    let reference = owner.dir.join("host-reference");
    fs::write(&reference, "owned").unwrap();
    owner.display = Some(sleep(&reference));
    pids.push(owner.display.as_ref().unwrap().id());
    let init = sleep(&reference);
    pids.push(init.id());
    owner.session = Some(Session {
        display: owner.display.take().unwrap(),
        init,
    });
}

#[test]
fn host_timeout_and_guest_exit_reap_both_children() {
    for timeout in [true, false] {
        let fixture = Fixture::new();
        let mut pids = Vec::new();
        let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
            host_session(owner, &mut pids);
            let session = owner.session.as_mut().unwrap();
            if !timeout {
                session.init.kill().unwrap();
                session.init.wait().unwrap();
            }
            check_boot(
                session,
                Instant::now() - Duration::from_secs(1),
                Duration::ZERO,
                None,
                None,
                None,
            )
        })
        .unwrap_err();
        assert!(
            error.starts_with(if timeout {
                "after 0ns:"
            } else {
                "guest-init exited:"
            }),
            "{error}"
        );
        assert_reaped(&pids);
        prepare_work(&fixture.work()).unwrap();
        assert!(!fixture.work().exists());
    }
}

#[test]
fn host_error_and_success_reap_both_children() {
    for fail in [true, false] {
        let fixture = Fixture::new();
        let mut pids = Vec::new();
        let result = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
            host_session(owner, &mut pids);
            if fail {
                aim_android_xml::read(b"").map(|_| ())
            } else {
                Ok(())
            }
        });
        assert_eq!(result.is_err(), fail);
        assert_reaped(&pids);
    }
}

#[test]
fn host_panic_reaps_children_and_removes_socket_directory() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let mut sockets = None;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TemplateBoot::run(
            &fixture.work().join("boot"),
            |owner| -> Result<(), String> {
                host_session(owner, &mut pids);
                owner.socket_dir = Some(DisplaySocketDir::new().unwrap());
                sockets = Some(owner.socket_dir.as_ref().unwrap().0.clone());
                panic!("capture panic");
            },
        )
    }));
    assert!(result.is_err());
    assert_reaped(&pids);
    assert!(!sockets.unwrap().exists());
}

#[test]
fn host_spawn_error_reaps_the_display() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
        let reference = fixture.0.join("host-reference");
        fs::write(&reference, "owned").unwrap();
        owner.display = Some(sleep(&reference));
        pids.push(owner.display.as_ref().unwrap().id());
        Command::new(fixture.0.join("missing-init"))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .unwrap_err();
    assert!(error.contains("No such file"), "{error}");
    assert_reaped(&pids);
}

#[test]
fn host_cleanup_failure_preserves_the_boot_error_and_live_lock() {
    use std::os::fd::AsRawFd;
    let fixture = Fixture::new();
    let dir = fixture.work().join("boot");
    fs::create_dir_all(dir.parent().unwrap()).unwrap();
    // Only the lock path is exercised; this file is never passed to an image tool.
    fs::write(data::image_of(&dir), "not an image").unwrap();
    let lock = fs::File::create(dir.with_extension("lock")).unwrap();
    // SAFETY: an owned file descriptor.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    assert!(data::remove(&dir).unwrap_err().contains("in use"));
    let error = TemplateBoot::run(&dir, |_| -> Result<(), String> {
        Err("capture failed".into())
    })
    .unwrap_err();
    assert!(
        error.starts_with("capture failed; cleanup failed:"),
        "{error}"
    );
    assert!(error.contains("in use"), "{error}");
    assert!(data::image_of(&dir).exists());
    fs::remove_file(data::image_of(&dir)).unwrap();
    drop(lock);
    // A concurrent test's fork can retain this descriptor until exec.
    fs::remove_file(dir.with_extension("lock")).unwrap();
}

fn assert_reaped(pids: &[u32]) {
    for &pid in pids {
        let mut status = 0;
        // SAFETY: only child pids this test started; the owner already reaped them.
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn timeout_detaches_after_reaping_and_next_run_is_clean() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
        session(owner, &mut pids);
        check_boot(
            owner.session.as_mut().unwrap(),
            Instant::now(),
            Duration::ZERO,
            None,
            None,
            None,
        )
    })
    .unwrap_err();
    assert!(
        error.contains("after 0ns: settings None, permissions None, sys.boot_completed None"),
        "{error}"
    );
    assert_reaped(&pids);
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn guest_failure_detaches_and_next_run_is_clean() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
        session(owner, &mut pids);
        let session = owner.session.as_mut().unwrap();
        session.init.kill().unwrap();
        session.init.wait().unwrap();
        check_boot(
            session,
            Instant::now(),
            DEFAULT_BOOT_TIMEOUT,
            None,
            None,
            None,
        )
    })
    .unwrap_err();
    assert!(error.starts_with("guest-init exited:"), "{error}");
    assert_reaped(&pids);
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn capture_error_detaches_and_next_run_is_clean() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
        session(owner, &mut pids);
        aim_android_xml::read(b"").map(|_| ())
    })
    .unwrap_err();
    assert!(error.contains("no root element"), "{error}");
    assert_reaped(&pids);
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn panic_detaches_after_reaping_and_next_run_is_clean() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        TemplateBoot::run(
            &fixture.work().join("boot"),
            |owner| -> Result<(), String> {
                session(owner, &mut pids);
                panic!("capture panic");
            },
        )
    }));
    assert!(result.is_err());
    assert_reaped(&pids);
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn successful_handoff_detaches_before_publication_reattaches() {
    let fixture = Fixture::new();
    let mut pids = Vec::new();
    let dir = fixture.work().join("boot");
    TemplateBoot::run(&dir, |owner| {
        session(owner, &mut pids);
        Ok(())
    })
    .unwrap();
    assert_reaped(&pids);
    fixture.assert_detached();
    let image = DataImage::attach(&dir, None).unwrap();
    assert_eq!(
        fs::read_to_string(image.dir().join("owned-reference")).unwrap(),
        "owned"
    );
    image.detach().unwrap();
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn spawn_error_reaps_the_display_and_detaches() {
    let fixture = Fixture::new();
    let mut pid = None;
    let error = TemplateBoot::run(&fixture.work().join("boot"), |owner| {
        let mut image = DataImage::attach(&owner.dir, None)?;
        let reference = image.dir().join("owned-reference");
        fs::write(&reference, "owned").unwrap();
        owner.display = Some(sleep(&reference));
        pid = Some(owner.display.as_ref().unwrap().id());
        image.handoff_detach()?;
        drop(image);
        Command::new(fixture.0.join("no-such-guest-init"))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("guest-init: {e}"))
    })
    .unwrap_err();
    assert!(error.starts_with("guest-init:"), "{error}");
    assert_reaped(&[pid.unwrap()]);
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn stale_work_detaches_exact_backings_and_preserves_another_image() {
    let fixture = Fixture::new();
    let other = DataImage::attach(&fixture.0.join("other"), None).unwrap();
    fs::write(other.dir().join("keep"), "other volume").unwrap();
    let other_attachments = disk::attachments_of(other.image()).unwrap();
    assert_eq!(other_attachments.len(), 1);
    for name in ["boot", "template"] {
        let mut image = DataImage::attach(&fixture.work().join(name), None).unwrap();
        image.handoff_detach().unwrap();
    }
    prepare_work(&fixture.work()).unwrap();
    assert!(!fixture.work().exists());
    assert_eq!(
        disk::attachments_of(other.image()).unwrap(),
        other_attachments
    );
    assert_eq!(
        fs::read_to_string(other.dir().join("keep")).unwrap(),
        "other volume"
    );
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn cleanup_refuses_a_live_owner_and_preserves_the_original_error() {
    let fixture = Fixture::new();
    let dir = fixture.work().join("boot");
    let image = DataImage::attach(&dir, None).unwrap();
    assert!(data::remove(&dir).unwrap_err().contains("in use"));
    let error = TemplateBoot::run(&dir, |_| -> Result<(), String> {
        Err("capture failed".into())
    })
    .unwrap_err();
    assert!(
        error.starts_with("capture failed; cleanup failed:"),
        "{error}"
    );
    assert!(error.contains("in use"), "{error}");
    assert!(disk::is_mount_point(&dir));
    image.detach().unwrap();
    fixture.next_run();
}

#[test]
#[ignore = "attaches disposable APFS images under target; no guest boot"]
fn publication_error_detaches_source_and_destination() {
    let fixture = Fixture::new();
    let empty = fixture.0.join("empty.asif");
    data::create(&empty).unwrap();
    let error = (|| {
        let from = DataImage::attach(&fixture.work().join("boot"), Some(&empty))?;
        let mut log = Log::create(fixture.0.join("publish.log"), false)?;
        publish(
            &[(from.dir(), &[PathBuf::from("missing-capture")])],
            &fixture.work(),
            &fixture.0.join("out"),
            "template.asif",
            &mut log,
            &empty,
        )
    })()
    .unwrap_err();
    assert!(error.contains("missing-capture"), "{error}");
    fixture.next_run();
}

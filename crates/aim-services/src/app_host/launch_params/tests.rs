use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

struct Data(PathBuf);
impl Data {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "aim-launch-params-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn user(&self, user: i32) {
        std::fs::create_dir_all(folder(&self.0, user).parent().unwrap()).unwrap();
    }
}
impl Drop for Data {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn name(package: &str) -> ComponentName {
    ComponentName {
        package: package.into(),
        class: format!("{package}.Main"),
    }
}

fn params() -> Params {
    Params {
        display_unique_id: Some("local:4619827259835644672".into()),
        windowing_mode: 5,
        bounds: Rect {
            left: -40,
            top: 60,
            right: 800,
            bottom: 1400,
        },
        window_layout_affinity: Some("shared<&layout".into()),
    }
}

fn records(owner: &Persister, user: i32) -> Records {
    let mut state = owner.state.lock().unwrap();
    state.finish_loading(user).unwrap();
    state.users.get(&user).cloned().unwrap_or_default()
}

// Block the real writer rather than racing its three-second delay.
fn block(owner: &Persister) -> mpsc::Sender<()> {
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    owner.queue.shared.barrier(entered, resume);
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    release
}

#[test]
fn launch_params_survive_task_owner_restart_in_original_abx() {
    let data = Data::new();
    data.user(0);
    let component = name("example.app");
    let path = file(&folder(&data.0, 0), &component);
    assert_eq!(path.file_name().unwrap(), "example.app-.Main.xml");
    {
        let tasks = super::super::tasks::Tasks::default();
        let owner = tasks.launch_params(&data.0).unwrap();
        owner
            .unlock_user(0, BTreeSet::from([component.package.clone()]))
            .unwrap();
        owner.save(0, &component, params()).unwrap();
        owner.queue.shared.flush();
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(aim_android_xml::abx::MAGIC));
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(root.name, "launch_params");
    assert_eq!(root.attrs.len(), 4);
    assert!(matches!(root.attr("windowing_mode"), Some(Value::Int(5))));
    assert_eq!(root.string("bounds").unwrap(), "-40 60 800 1400");
    assert!(!path.with_extension("xml.new").exists());
    let tasks = super::super::tasks::Tasks::default();
    let owner = tasks.launch_params(&data.0).unwrap();
    owner
        .unlock_user(0, BTreeSet::from([component.package.clone()]))
        .unwrap();
    assert_eq!(owner.get(0, &component).unwrap(), Some(params()));
}

#[test]
fn launch_params_package_removal_callback_cleans_all_loaded_users() {
    crate::system::with_discovery_session(|host, _, _| {
        let data = host.data().unwrap();
        let owner = host.tasks.launch_params(data).unwrap();
        let removed = name("removed.app");
        let kept = name("removed.app.other");
        for user in [0, 10] {
            std::fs::create_dir_all(folder(data, user).parent().unwrap()).unwrap();
            owner.save(user, &removed, params()).unwrap();
            owner.save(user, &kept, params()).unwrap();
        }
        owner.queue.shared.flush();
        Observer(Arc::downgrade(host))
            .package_removed(Some("removed.app"), 10123)
            .unwrap();
        owner.queue.shared.flush();
        for user in [0, 10] {
            assert!(!records(&owner, user).contains_key(&removed));
            assert!(!file(&folder(data, user), &removed).exists());
            assert_eq!(records(&owner, user).get(&kept), Some(&params()));
            assert!(file(&folder(data, user), &kept).exists());
        }
    });
}

#[test]
fn launch_params_removal_cancels_coalesced_writes_and_preserves_queue_order() {
    let data = Data::new();
    for user in [0, 10] {
        data.user(user);
    }
    let owner = Persister::new(&data.0).unwrap();
    let release = block(&owner);
    let removed = name("removed");
    let kept = name("kept");
    for user in [0, 10] {
        owner.save(user, &removed, params()).unwrap();
        owner.save(user, &kept, params()).unwrap();
        let mut changed = params();
        changed.bounds.left = 100;
        owner.save(user, &removed, changed.clone()).unwrap();
        owner.save(user, &kept, changed.clone()).unwrap();
        // The pin's queued item refers to the mutable record even when an
        // affinity-only change doesn't request another write.
        changed.window_layout_affinity = Some("latest".into());
        owner.save(user, &kept, changed).unwrap();
    }
    assert_eq!(owner.queue.shared.pending_writes(), 4);
    owner.remove_package("removed").unwrap();
    assert_eq!(owner.queue.shared.pending_writes(), 2);
    // A later reinstall/save must run after the earlier deletion, and survive.
    owner.save(10, &removed, params()).unwrap();
    release.send(()).unwrap();
    owner.queue.shared.flush();
    assert!(!file(&folder(&data.0, 0), &removed).exists());
    assert!(file(&folder(&data.0, 10), &removed).exists());
    for user in [0, 10] {
        let bytes = std::fs::read(file(&folder(&data.0, user), &kept)).unwrap();
        let latest = Params::restore(&bytes).unwrap();
        assert_eq!(latest.bounds.left, 100);
        assert_eq!(latest.window_layout_affinity.as_deref(), Some("latest"));
    }
}

#[test]
fn launch_params_removal_deletes_a_write_already_taken_by_the_writer() {
    let data = Data::new();
    data.user(0);
    let owner = Persister::new(&data.0).unwrap();
    let component = name("removed");
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    owner.queue.shared.pause_next_write(entered, resume);
    owner.save(0, &component, params()).unwrap();
    owner.queue.shared.cleanup(Vec::new());
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(owner.queue.shared.pending_writes(), 0);
    owner.remove_package("removed").unwrap();
    release.send(()).unwrap();
    owner.queue.shared.flush();
    assert!(records(&owner, 0).is_empty());
    assert!(!file(&folder(&data.0, 0), &component).exists());
}

#[test]
fn launch_params_removal_waits_for_unlock_loads() {
    let data = Data::new();
    for user in [0, 10] {
        data.user(user);
    }
    let component = name("removed");
    {
        let owner = Persister::new(&data.0).unwrap();
        for user in [0, 10] {
            owner.save(user, &component, params()).unwrap();
        }
    }
    let owner = Persister::new(&data.0).unwrap();
    for user in [0, 10] {
        owner
            .unlock_user(user, BTreeSet::from(["removed".into()]))
            .unwrap();
    }
    owner.remove_package("removed").unwrap();
    owner.queue.shared.flush();
    for user in [0, 10] {
        assert!(records(&owner, user).is_empty());
        assert!(!file(&folder(&data.0, user), &component).exists());
    }
}

#[test]
fn launch_params_load_migrates_text_xml_and_cleans_invalid_and_uninstalled_files() {
    let data = Data::new();
    data.user(0);
    let dir = folder(&data.0, 0);
    std::fs::create_dir(&dir).unwrap();
    let text =
        br#"<launch_params display_unique_id="external:one" windowing_mode="1" bounds="1 2 3 4"/>"#;
    std::fs::write(dir.join("kept_.Main.xml"), text).unwrap();
    for (file, bytes) in [
        ("gone-.Main.xml", text.as_slice()),
        ("ambiguous_package_.Main.xml", text.as_slice()),
        ("kept-.Bad.xml", b"<launch_params windowing_mode=\"bad\"/>"),
        ("unfinished.xml.new", b"partial"),
        ("invalid.xml", text.as_slice()),
    ] {
        std::fs::write(dir.join(file), bytes).unwrap();
    }
    let owner = Persister::new(&data.0).unwrap();
    owner
        .unlock_user(
            0,
            BTreeSet::from(["kept".into(), "ambiguous_package".into()]),
        )
        .unwrap();
    let loaded = records(&owner, 0);
    assert_eq!(loaded.len(), 1);
    let p = loaded.get(&name("kept")).unwrap();
    assert_eq!(p.display_unique_id.as_deref(), Some("external:one"));
    assert_eq!(p.windowing_mode, 1);
    assert_eq!(
        p.bounds,
        Rect {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4
        }
    );
    assert_eq!(p.window_layout_affinity, None);
    owner.queue.shared.flush();
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    assert!(dir.join("kept-.Main.xml").exists());
}

#[test]
fn launch_params_restore_follows_rect_and_integer_parsing() {
    for bounds in ["bad", "1 2 3", "+1 2 3 4", "1  2 3 4"] {
        let xml = format!("<launch_params bounds=\"{bounds}\"/>");
        assert_eq!(
            Params::restore(xml.as_bytes()).unwrap().bounds,
            Rect::default()
        );
    }
    assert!(Params::restore(b"<launch_params windowing_mode=\"bad\"/>").is_err());
    assert_eq!(Params::restore(b"<unrelated/>").unwrap(), Params::default());
}

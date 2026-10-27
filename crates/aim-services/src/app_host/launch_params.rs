//! LaunchParamsPersister at android-16.0.0_r1: per-user, per-real-activity
//! records in /data/system_ce/<user>/launch_params. The task owner serializes
//! record changes and removal; one PersisterQueue writer orders atomic writes
//! and cleanup, including deletion after a write already in flight.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

mod queue;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Weak};
use std::thread::JoinHandle;

use aim_android_xml::{Element, Value};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::AppHost;
use super::config::Rect;
use super::package_observers::PackageListObserver;
use crate::package::intent::ComponentName;

use queue::Queue;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Params {
    pub display_unique_id: Option<String>,
    pub windowing_mode: i32,
    pub bounds: Rect,
    pub window_layout_affinity: Option<String>,
}

impl Params {
    fn xml(&self) -> Result<Vec<u8>, String> {
        let b = self.bounds;
        let mut attrs = vec![
            (
                "display_unique_id".into(),
                self.display_unique_id
                    .clone()
                    .map_or(Value::Null, Value::String),
            ),
            ("windowing_mode".into(), Value::Int(self.windowing_mode)),
            (
                "bounds".into(),
                Value::String(format!("{} {} {} {}", b.left, b.top, b.right, b.bottom)),
            ),
        ];
        if let Some(affinity) = &self.window_layout_affinity {
            attrs.push((
                "window_layout_affinity".into(),
                Value::String(affinity.clone()),
            ));
        }
        aim_android_xml::abx::write(&Element {
            name: "launch_params".into(),
            attrs,
            content: Vec::new(),
        })
    }

    fn restore(bytes: &[u8]) -> Result<Self, String> {
        let mut params = Self::default();
        if let Some(root) = aim_android_xml::read_next_optional(bytes)?
            && root.name == "launch_params"
        {
            params.display_unique_id = root.string("display_unique_id").map(|s| s.into_owned());
            params.windowing_mode = root
                .string("windowing_mode")
                .map(|s| s.parse::<i32>().map_err(|e| e.to_string()))
                .transpose()?
                .unwrap_or(0);
            // Rect.unflattenFromString leaves the empty Rect on invalid input.
            if let Some(bounds) = root.string("bounds") {
                let values: Option<Vec<i32>> = bounds
                    .split(' ')
                    .map(|s| {
                        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit() || b == b'-'))
                            .then(|| s.parse().ok())
                            .flatten()
                    })
                    .collect();
                if let Some(values) = values
                    && let [left, top, right, bottom] = values.as_slice()
                {
                    params.bounds = Rect {
                        left: *left,
                        top: *top,
                        right: *right,
                        bottom: *bottom,
                    };
                }
            }
            params.window_layout_affinity = root
                .string("window_layout_affinity")
                .map(|s| s.into_owned());
        }
        Ok(params)
    }
}

type Records = BTreeMap<ComponentName, Params>;

#[derive(Default)]
struct State {
    users: BTreeMap<i32, Records>,
    loading: BTreeMap<i32, JoinHandle<Result<Option<Records>, String>>>,
}

impl State {
    fn finish_loading(&mut self, user: i32) -> Result<(), String> {
        if let Some(task) = self.loading.remove(&user) {
            let records = task
                .join()
                .map_err(|_| format!("launch params loader for {user} panicked"))??;
            if let Some(records) = records {
                self.users.insert(user, records);
            }
        }
        Ok(())
    }
}

pub(super) struct Persister {
    data: PathBuf,
    state: Mutex<State>,
    queue: Queue,
}

fn folder(data: &Path, user: i32) -> PathBuf {
    data.join("system_ce")
        .join(user.to_string())
        .join("launch_params")
}

fn file(folder: &Path, name: &ComponentName) -> PathBuf {
    let class = name
        .class
        .strip_prefix(&name.package)
        .filter(|c| c.starts_with('.'))
        .unwrap_or(&name.class);
    folder.join(format!("{}-{class}.xml", name.package))
}

impl Persister {
    #[cfg(test)]
    pub fn flush(&self) {
        self.queue.shared.flush();
    }

    pub fn new(data: &Path) -> Result<Self, String> {
        Ok(Self {
            data: data.to_owned(),
            state: Mutex::default(),
            queue: Queue::new()?,
        })
    }

    pub fn unlock_user(&self, user: i32, packages: BTreeSet<String>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.loading.contains_key(&user) {
            eprintln!("app host: launch params duplicate unlock for {user}");
            return Ok(());
        }
        let folder = folder(&self.data, user);
        let queue = self.queue.shared.clone();
        let task = std::thread::Builder::new()
            .name(format!("launch-params-load-{user}"))
            .spawn(move || load(&folder, &packages, &queue))
            .map_err(|e| e.to_string())?;
        state.loading.insert(user, task);
        Ok(())
    }

    pub fn save(&self, user: i32, name: &ComponentName, params: Params) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.finish_loading(user)?;
        let records = state.users.entry(user).or_default();
        let previous = records.entry(name.clone()).or_default();
        // Preserve the pin's Objects.equals (without negation) for affinity.
        let changed = previous.display_unique_id != params.display_unique_id
            || previous.windowing_mode != params.windowing_mode
            || previous.bounds != params.bounds
            || previous.window_layout_affinity == params.window_layout_affinity;
        *previous = params.clone();
        self.queue.shared.write(
            name.clone(),
            file(&folder(&self.data, user), name),
            params,
            changed,
        );
        Ok(())
    }

    /// `LaunchParamsPersister.getLaunchParams`, before launch placement.
    pub fn get(&self, user: i32, name: &ComponentName) -> Result<Option<Params>, String> {
        let mut state = self.state.lock().unwrap();
        state.finish_loading(user)?;
        Ok(state.users.get(&user).and_then(|m| m.get(name)).cloned())
    }

    pub fn remove_package(&self, package: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        // Join pending unlock loads before removal, so they cannot later put
        // a removed record back in the owner's map.
        let loading: Vec<_> = state.loading.keys().copied().collect();
        for user in loading {
            state.finish_loading(user)?;
        }
        let mut files = Vec::new();
        for (&user, records) in &mut state.users {
            records.retain(|name, _| {
                if name.package == package {
                    files.push(file(&folder(&self.data, user), name));
                    false
                } else {
                    true
                }
            });
        }
        self.queue.shared.remove_package(package, files);
        Ok(())
    }
}

impl Drop for Persister {
    fn drop(&mut self) {
        for (user, task) in std::mem::take(&mut self.state.get_mut().unwrap().loading) {
            match task.join() {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("app host: launch params user {user}: {e}"),
                Err(_) => eprintln!("app host: launch params loader for {user} panicked"),
            }
        }
        // Queue::drop drains and joins the writer after all loaders finish.
    }
}

fn load(
    folder: &Path,
    packages: &BTreeSet<String>,
    queue: &queue::Shared,
) -> Result<Option<Records>, String> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", folder.display())),
    };
    let mut records = Records::new();
    let mut cleanup = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let mut path = entry.path();
        if !entry.metadata().map_err(|e| e.to_string())?.is_file() {
            eprintln!("app host: launch params {} is not a file", path.display());
            continue;
        }
        let mut filename = entry.file_name().to_string_lossy().into_owned();
        if !filename.ends_with(".xml") || filename.matches('_').count() > 1 {
            cleanup.push(path);
            continue;
        }
        if filename.contains('_') {
            filename = filename.replace('_', "-");
            let new = folder.join(&filename);
            if let Err(e) = std::fs::rename(&path, &new) {
                eprintln!("app host: launch params migrate {}: {e}", path.display());
                cleanup.push(path);
                continue;
            }
            path = new;
        }
        let name =
            crate::package::preferred::unflatten(&filename[..filename.len() - 4].replace('-', "/"));
        let Some(name) = name.filter(|n| packages.contains(&n.package)) else {
            cleanup.push(path);
            continue;
        };
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| Params::restore(&b))
        {
            Ok(params) => {
                records.insert(name, params);
            }
            Err(e) => {
                eprintln!("app host: launch params restore {}: {e}", path.display());
                cleanup.push(path);
            }
        }
    }
    if !cleanup.is_empty() {
        queue.cleanup(cleanup);
    }
    Ok(Some(records))
}

pub(super) struct Observer(pub Weak<AppHost>);

impl PackageListObserver for Observer {
    fn package_added(&self, _: Option<&str>, _: i32) -> Result<(), Exception> {
        Ok(())
    }
    fn package_changed(&self, _: Option<&str>, _: i32) -> Result<(), Exception> {
        Ok(())
    }
    fn package_removed(&self, name: Option<&str>, _: i32) -> Result<(), Exception> {
        let host = self
            .0
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host gone"))?;
        let Some(name) = name else {
            return Ok(());
        };
        host.tasks
            .launch_params(host.data()?)?
            .remove_package(name)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
    }
}

#[cfg(test)]
mod tests;

//! PersisterQueue's coalescing, delays and flush ordering for launch params.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{ComponentName, Params};

const PRE_TASK_DELAY: Duration = Duration::from_secs(3);
const INTER_WRITE_DELAY: Duration = Duration::from_millis(500);
const MAX_WRITE_QUEUE_LENGTH: usize = 6;

enum Item {
    Write {
        name: ComponentName,
        path: PathBuf,
        params: Params,
    },
    Delete(Vec<PathBuf>),
    #[cfg(test)]
    Barrier(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>),
}

impl Item {
    fn process(self) {
        match self {
            Self::Write { path, params, .. } => {
                if let Err(e) = write(&path, &params) {
                    eprintln!("app host: launch params write {}: {e}", path.display());
                }
            }
            Self::Delete(files) => {
                for path in files {
                    if let Err(e) = std::fs::remove_file(&path) {
                        eprintln!("app host: launch params delete {}: {e}", path.display());
                    }
                }
            }
            #[cfg(test)]
            Self::Barrier(entered, release) => {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
        }
    }
}

#[derive(Default)]
struct State {
    items: VecDeque<Item>,
    next: Option<Instant>,
    flush: bool,
    stop: bool,
    active: bool,
}

#[derive(Default)]
pub(super) struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    #[cfg(test)]
    before_write: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

impl Shared {
    fn push(&self, state: &mut State, item: Item) {
        state.flush |= !matches!(item, Item::Write { .. });
        state.items.push_back(item);
        state.flush |= state.items.len() > MAX_WRITE_QUEUE_LENGTH;
        state
            .next
            .get_or_insert_with(|| Instant::now() + PRE_TASK_DELAY);
        self.changed.notify_all();
    }

    pub fn cleanup(&self, files: Vec<PathBuf>) {
        self.push(&mut self.state.lock().unwrap(), Item::Delete(files));
    }

    pub fn write(&self, name: ComponentName, path: PathBuf, params: Params, changed: bool) {
        let mut state = self.state.lock().unwrap();
        for item in state.items.iter_mut().rev() {
            if let Item::Write {
                path: queued_path,
                params: queued,
                ..
            } = item
                && *queued_path == path
            {
                *queued = params;
                return;
            }
        }
        if !changed {
            return;
        }
        self.push(&mut state, Item::Write { name, path, params });
    }

    pub fn remove_package(&self, package: &str, files: Vec<PathBuf>) {
        let mut state = self.state.lock().unwrap();
        state
            .items
            .retain(|item| !matches!(item, Item::Write { name, .. } if name.package == package));
        self.push(&mut state, Item::Delete(files));
    }

    fn run(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.items.is_empty() {
                state.next = None;
                state.flush = false;
                state.active = false;
                self.changed.notify_all();
                if state.stop {
                    return;
                }
                state = self.changed.wait(state).unwrap();
                continue;
            }
            if !state.flush {
                let delay = state
                    .next
                    .unwrap()
                    .saturating_duration_since(Instant::now());
                if !delay.is_zero() {
                    state = self.changed.wait_timeout(state, delay).unwrap().0;
                    continue;
                }
            }
            let item = state.items.pop_front().unwrap();
            state.active = true;
            drop(state);
            #[cfg(test)]
            if matches!(item, Item::Write { .. })
                && let Some((entered, release)) = self.before_write.lock().unwrap().take()
            {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            item.process();
            state = self.state.lock().unwrap();
            state.active = false;
            state.next = Some(Instant::now() + INTER_WRITE_DELAY);
        }
    }

    #[cfg(test)]
    pub fn barrier(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        self.push(
            &mut self.state.lock().unwrap(),
            Item::Barrier(entered, release),
        );
    }

    #[cfg(test)]
    pub fn flush(&self) {
        let mut state = self.state.lock().unwrap();
        state.flush = true;
        self.changed.notify_all();
        while state.active || !state.items.is_empty() {
            let (next, timeout) = self
                .changed
                .wait_timeout(state, Duration::from_secs(10))
                .unwrap();
            state = next;
            assert!(!timeout.timed_out(), "launch params writer did not drain");
        }
    }

    #[cfg(test)]
    pub fn pending_writes(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .items
            .iter()
            .filter(|i| matches!(i, Item::Write { .. }))
            .count()
    }

    #[cfg(test)]
    pub fn pause_next_write(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.before_write.lock().unwrap() = Some((entered, release));
    }
}

pub(super) struct Queue {
    pub shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Queue {
    pub fn new() -> Result<Self, String> {
        let shared = Arc::new(Shared::default());
        let writer = shared.clone();
        let worker = std::thread::Builder::new()
            .name("launch-params-writer".into())
            .spawn(move || writer.run())
            .map_err(|e| e.to_string())?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.stop = true;
            state.flush = true;
            self.shared.changed.notify_all();
        }
        if self.worker.take().unwrap().join().is_err() {
            eprintln!("app host: launch params writer panicked");
        }
    }
}

fn write(path: &std::path::Path, params: &Params) -> Result<(), String> {
    let bytes = params.xml()?;
    let folder = path.parent().unwrap();
    if !folder.is_dir() {
        std::fs::create_dir(folder).map_err(|e| e.to_string())?;
        aim_storage::guest_inode::record(
            folder,
            aim_storage::guest_inode::GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o700),
            },
        )
        .map_err(|e| e.to_string())?;
    }
    super::super::usage_database::atomic_write(path, &bytes).map_err(|e| {
        let new = path.with_extension("xml.new");
        if let Err(cleanup) = std::fs::remove_file(&new)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "app host: launch params failWrite {}: {cleanup}",
                new.display()
            );
        }
        e.to_string()
    })
}

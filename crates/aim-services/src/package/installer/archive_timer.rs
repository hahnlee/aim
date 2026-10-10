//! Owned 120-second draft expiry handler; no detached work survives shutdown.
use super::archiver::Drafts;
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex, Weak},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
struct State {
    stop: bool,
    deadlines: BTreeMap<i32, Instant>,
}
pub struct Timer {
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}
impl Timer {
    pub fn new(drafts: Weak<Drafts>) -> Self {
        let state = Arc::new((
            Mutex::new(State {
                stop: false,
                deadlines: BTreeMap::new(),
            }),
            Condvar::new(),
        ));
        let shared = state.clone();
        let worker = thread::spawn(move || {
            loop {
                let (lock, wake) = &*shared;
                let mut state = lock.lock().unwrap();
                if state.stop {
                    break;
                }
                let next = state
                    .deadlines
                    .iter()
                    .min_by_key(|(_, time)| *time)
                    .map(|(&id, &time)| (id, time));
                let Some((id, time)) = next else {
                    drop(wake.wait(state).unwrap());
                    continue;
                };
                let now = Instant::now();
                if now < time {
                    drop(wake.wait_timeout(state, time - now).unwrap());
                    continue;
                }
                state.deadlines.remove(&id);
                drop(state);
                let Some(drafts) = drafts.upgrade() else {
                    break;
                };
                if let Err(error) = drafts.expire(id) {
                    eprintln!("native unarchive draft expiry failed: {}", error.message);
                }
            }
        });
        Self {
            state,
            worker: Some(worker),
        }
    }
    pub fn schedule(&self, id: i32) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        if !state.stop {
            state
                .deadlines
                .insert(id, Instant::now() + Duration::from_secs(120));
            wake.notify_one();
        }
    }
}
impl Drop for Timer {
    fn drop(&mut self) {
        let (lock, wake) = &*self.state;
        {
            let mut state = lock.lock().unwrap();
            state.stop = true;
            state.deadlines.clear();
            wake.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            if worker.thread().id() != thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

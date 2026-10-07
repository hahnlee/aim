//! Owned runtime persistence timer. Drop stops and joins its thread.
use super::{FlushError, State};
use std::{
    sync::{Arc, Condvar, Mutex, Weak},
    thread::{self, JoinHandle},
    time::Instant,
};
#[derive(Clone, Debug)]
pub(super) struct Wake(Arc<(Mutex<(u64, bool)>, Condvar)>);
impl PartialEq for Wake {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Wake {}
impl Wake {
    pub fn notify(&self) {
        let (lock, cv) = &*self.0;
        let mut state = lock.lock().unwrap();
        state.0 = state.0.wrapping_add(1);
        cv.notify_all();
    }
}
/// Bootstrap teardown signals cancellation without joining under its lock.
#[derive(Clone)]
pub struct StopHandle(Wake);
impl StopHandle {
    pub fn stop(&self) {
        let (lock, cv) = &*self.0.0;
        lock.lock().unwrap().1 = true;
        cv.notify_all();
    }
}
pub struct Worker {
    wake: Wake,
    metadata: Weak<Mutex<State>>,
    thread: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<FlushError>>>,
}
impl Worker {
    pub fn start(
        metadata: &Arc<Mutex<State>>,
        mut persist: impl FnMut(Instant) -> Result<Vec<u32>, FlushError> + Send + 'static,
    ) -> std::io::Result<Self> {
        let wake = Wake(Arc::new((Mutex::new((0, false)), Condvar::new())));
        {
            let mut state = metadata.lock().unwrap();
            if state.wake.is_some() {
                return Err(std::io::Error::other("runtime timer already attached"));
            }
            state.wake = Some(wake.clone());
        }
        let weak = Arc::downgrade(metadata);
        let owner = weak.clone();
        let timer_wake = wake.clone();
        let error = Arc::new(Mutex::new(None));
        let errors = error.clone();
        let spawn = thread::Builder::new()
            .name("package-runtime-writes".into())
            .spawn(move || {
                let mut failed_generation = None;
                loop {
                    let (lock, cv) = &*timer_wake.0;
                    let generation = {
                        let state = lock.lock().unwrap();
                        if state.1 {
                            break;
                        }
                        state.0
                    };
                    let Some(metadata) = weak.upgrade() else {
                        break;
                    };
                    let deadline = metadata.lock().unwrap().next_write_deadline();
                    drop(metadata);
                    let mut signal = lock.lock().unwrap();
                    if signal.1 {
                        break;
                    }
                    if signal.0 != generation {
                        continue;
                    }
                    if failed_generation == Some(generation) || deadline.is_none() {
                        signal = cv.wait(signal).unwrap();
                        drop(signal);
                        continue;
                    }
                    let deadline = deadline.unwrap();
                    let now = Instant::now();
                    if deadline > now {
                        let (signal, _) = cv.wait_timeout(signal, deadline - now).unwrap();
                        drop(signal);
                        continue;
                    }
                    drop(signal);
                    match persist(now) {
                        Ok(_) => failed_generation = None,
                        Err(error) => {
                            eprintln!("services: runtime permission timer: {error}");
                            *errors.lock().unwrap() = Some(error);
                            failed_generation = Some(generation);
                        }
                    }
                }
            });
        match spawn {
            Ok(thread) => Ok(Self {
                wake,
                metadata: owner,
                thread: Some(thread),
                error,
            }),
            Err(error) => {
                metadata.lock().unwrap().wake = None;
                Err(error)
            }
        }
    }
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.wake.clone())
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
    /// Explicit wake after an external failure is repaired; no busy retry loop.
    pub fn retry(&self) {
        self.wake.notify();
    }
    pub fn take_error(&self) -> Option<FlushError> {
        self.error.lock().unwrap().take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop_handle().stop();
        if let Some(thread) = self.thread.take() {
            thread.join().expect("runtime timer panicked");
        }
        if let Some(metadata) = self.metadata.upgrade() {
            let mut state = metadata.lock().unwrap();
            if state.wake.as_ref() == Some(&self.wake) {
                state.wake = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};
    #[test]
    fn bootstrap_stop_wakes_idle_worker_without_joining_under_owner_lock() {
        let metadata = Arc::new(Mutex::new(State::default()));
        let worker = Worker::start(&metadata, |_| panic!("idle worker must not write")).unwrap();
        worker.stop_handle().stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        drop(worker);
        assert!(metadata.lock().unwrap().wake.is_none());
    }

    #[test]
    fn worker_wakes_on_mutation_reports_failure_retries_and_restarts_after_join() {
        let metadata = Arc::new(Mutex::new(State::default()));
        let source = metadata.clone();
        let (tx, rx) = mpsc::channel();
        let mut attempts = 0;
        let worker = Worker::start(&metadata, move |now| {
            attempts += 1;
            if attempts == 1 {
                tx.send(false).unwrap();
                return Err(FlushError {
                    user: 0,
                    completed: vec![],
                    error: super::super::super::WriteError {
                        committed: false,
                        message: "injected producer failure".into(),
                    },
                });
            }
            let result = source
                .lock()
                .unwrap()
                .flush_due_with(now, |user, _| Ok(user as u32));
            tx.send(true).unwrap();
            result
        })
        .unwrap();
        metadata.lock().unwrap().set_version(0, 7);
        assert_eq!(rx.recv_timeout(Duration::from_secs(4)).unwrap(), false);
        let deadline = Instant::now() + Duration::from_secs(2);
        let error = loop {
            if let Some(error) = worker.take_error() {
                break error;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(error.user, 0);
        assert_eq!(metadata.lock().unwrap().pending_write_requests(), [0]);
        worker.retry();
        assert!(rx.recv_timeout(Duration::from_secs(4)).unwrap());
        assert!(metadata.lock().unwrap().pending_write_requests().is_empty());
        drop(worker);
        assert!(metadata.lock().unwrap().wake.is_none());
        let worker = Worker::start(&metadata, |_| Ok(vec![])).unwrap();
        drop(worker);
        assert!(metadata.lock().unwrap().wake.is_none());
    }
}

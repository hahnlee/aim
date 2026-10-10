//! Native PMS foreground/background handlers and waitForHandler barriers.
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct Queue {
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    stopping: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// Retain outside service owner locks; drop stops and joins this handler.
pub struct Worker {
    queue: Arc<Queue>,
    thread: Option<JoinHandle<()>>,
}

impl Queue {
    pub fn new(name: &str) -> Result<Arc<Self>, Exception> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    job();
                }
            })
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?;
        Ok(Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            stopping,
            thread: Mutex::new(Some(thread)),
        }))
    }

    pub fn post(&self, job: impl FnOnce() + Send + 'static) -> Result<(), Exception> {
        self.sender
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "package handler stopped"))?
            .send(Box::new(job))
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))
    }

    /// The barrier is queued even when the caller supplies a nonpositive timeout.
    pub fn wait(&self, timeout_millis: i64) -> Result<bool, Exception> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.post(move || {
            let _ = sender.send(());
        })?;
        if receiver.try_recv().is_ok() {
            return Ok(true);
        }
        if timeout_millis <= 0 {
            return Ok(false);
        }
        match receiver.recv_timeout(Duration::from_millis(timeout_millis as u64)) {
            Ok(()) => Ok(true),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package handler stopped before barrier",
            )),
        }
    }

    pub fn take_worker(self: &Arc<Self>) -> Option<Worker> {
        self.thread.lock().unwrap().take().map(|thread| Worker {
            queue: self.clone(),
            thread: Some(thread),
        })
    }

    /// Nonjoining shutdown is safe while detaching bootstrap state.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.sender.lock().unwrap().take();
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        self.shutdown();
        self.thread.lock().unwrap().take();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.queue.shutdown();
        if let Some(thread) = self.thread.take() {
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

pub struct Handlers {
    pub foreground: Arc<Queue>,
    pub background: Arc<Queue>,
}
impl Handlers {
    pub fn new() -> Result<Self, Exception> {
        Ok(Self {
            foreground: Queue::new("package-handler")?,
            background: Queue::new("package-background-handler")?,
        })
    }
    pub fn wait(&self, timeout_millis: i64, background: bool) -> Result<bool, Exception> {
        if background {
            &self.background
        } else {
            &self.foreground
        }
        .wait(timeout_millis)
    }
    pub fn shutdown(&self) {
        self.foreground.shutdown();
        self.background.shutdown();
    }
}

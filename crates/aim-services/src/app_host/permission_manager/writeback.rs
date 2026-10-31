//! AccessPersistence's delayed, bounded, atomic user-state writes (Android 16 r1).
use super::*;
use std::{
    sync::Condvar,
    thread::JoinHandle,
    time::{Duration, Instant},
};

type Write = Box<dyn Fn() -> Result<(), Exception> + Send>;
#[derive(Default)]
struct Pending {
    write: Option<Write>,
    first: Option<Instant>,
    next: Option<Instant>,
    stopped: bool,
}
#[derive(Default)]
pub(super) struct Writer {
    shared: Arc<(Mutex<Pending>, Condvar)>,
    worker: Mutex<Option<JoinHandle<()>>>,
}
impl Pending {
    fn flush(&mut self) -> Result<(), Exception> {
        if let Some(write) = &self.write {
            write()?;
        }
        self.write = None;
        self.first = None;
        self.next = None;
        Ok(())
    }
}
impl Writer {
    pub(super) fn schedule(&self, write: Write) -> Result<(), Exception> {
        let mut worker = self.worker.lock().unwrap();
        let (lock, cv) = &*self.shared;
        let mut pending = lock.lock().unwrap();
        if pending.stopped {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "permission writer stopped",
            ));
        }
        if worker.is_none() {
            let shared = self.shared.clone();
            *worker = Some(
                std::thread::Builder::new()
                    .name("permission-writes".into())
                    .spawn(move || {
                        let (lock, cv) = &*shared;
                        let mut pending = lock.lock().unwrap();
                        loop {
                            if pending.stopped {
                                break;
                            }
                            match pending.next {
                                None => pending = cv.wait(pending).unwrap(),
                                Some(at) if at > Instant::now() => {
                                    pending = cv
                                        .wait_timeout(
                                            pending,
                                            at.saturating_duration_since(Instant::now()),
                                        )
                                        .unwrap()
                                        .0;
                                }
                                Some(_) => {
                                    if let Err(error) = pending.flush() {
                                        eprintln!(
                                            "app host: permission persistence: {}",
                                            error.message
                                        );
                                        pending.next =
                                            Some(Instant::now() + Duration::from_secs(1));
                                    }
                                }
                            }
                        }
                    })
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))?,
            );
        }
        let now = Instant::now();
        let first = *pending.first.get_or_insert(now);
        pending.next = Some((now + Duration::from_secs(1)).min(first + Duration::from_secs(2)));
        pending.write = Some(write);
        cv.notify_all();
        Ok(())
    }
    // Synchronous replacements cancel an older queued snapshot under the same IO lock.
    pub(super) fn synchronize(
        &self,
        write: impl FnOnce() -> Result<(), Exception>,
    ) -> Result<(), Exception> {
        let mut pending = self.shared.0.lock().unwrap();
        write()?;
        pending.write = None;
        pending.first = None;
        pending.next = None;
        self.shared.1.notify_all();
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn flush(&self) -> Result<(), Exception> {
        self.shared.0.lock().unwrap().flush()
    }

    pub(super) fn close(&self) -> Result<(), Exception> {
        let mut worker = self.worker.lock().unwrap();
        let mut pending = self.shared.0.lock().unwrap();
        let result = pending.flush();
        pending.stopped = true;
        self.shared.1.notify_all();
        drop(pending);
        if let Some(worker) = worker.take() {
            worker
                .join()
                .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "permission writer panicked"))?;
        }
        result
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("app host: permission persistence close: {}", error.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn delayed_writes_coalesce_and_close_flushes_the_latest_state() {
        let writer = Writer::default();
        let (tx, rx) = mpsc::channel();
        for value in 0..100 {
            let tx = tx.clone();
            writer
                .schedule(Box::new(move || {
                    tx.send(value).unwrap();
                    Ok(())
                }))
                .unwrap();
        }
        assert!(rx.try_recv().is_err());
        let pending = writer.shared.0.lock().unwrap();
        assert!(pending.next.unwrap() <= pending.first.unwrap() + Duration::from_secs(2));
        drop(pending);
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), 99);
        assert!(rx.try_recv().is_err());
        writer
            .schedule(Box::new(move || {
                tx.send(100).unwrap();
                Ok(())
            }))
            .unwrap();
        writer.close().unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 100);
        assert!(writer.schedule(Box::new(|| Ok(()))).is_err());
    }

    #[test]
    fn failed_writes_remain_pending_and_synchronous_replacements_cannot_regress() {
        let writer = Writer::default();
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        writer
            .schedule(Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                Err(Exception::new(EX_ILLEGAL_STATE, "disk failure"))
            }))
            .unwrap();
        assert_eq!(writer.flush().unwrap_err().message, "disk failure");
        assert!(writer.shared.0.lock().unwrap().write.is_some());
        writer.synchronize(|| Ok(())).unwrap();
        writer.close().unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}

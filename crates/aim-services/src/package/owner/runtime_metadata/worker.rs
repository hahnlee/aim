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
    fn running_worker_uses_new_users_actual_creation_metadata_and_wakes_after_handoff(){
        use crate::package::{owner::{Store,tests::Data,WriteError},permissions::RuntimePermissions};
        use aim_storage::guest_inode::{self,GuestInode};
        use std::{fs,os::unix::fs::{OpenOptionsExt,MetadataExt}};
        let data=Data::new();data.settings();
        let mut disk=Store::open(&data.0,&[0]).unwrap().unwrap();disk.claim_runtime_permission_inventory(&[0]).unwrap();disk.register_package_user(10).unwrap();
        let disk=Arc::new(Mutex::new(disk));let metadata=Arc::new(Mutex::new(State::default()));
        let initial:[(u32,GuestInode);1]=[(0,GuestInode{uid:Some(1000),gid:Some(1000),mode:Some(0o660)})];
        metadata.lock().unwrap().bind_creation_metadata(&initial.into()).unwrap();
        let source=metadata.clone();let output=disk.clone();let (tx,rx)=mpsc::channel();
        let worker=Worker::start(&metadata,move|now|{
            let result=source.lock().unwrap().flush_due_with(now,|user,current|{
                let inode=current.creation_inode(user as u32).ok_or_else(||WriteError{committed:false,message:"actual new-user creation metadata is absent".into()})?;
                let desired=RuntimePermissions{version:current.version(user),fingerprint:current.fingerprint(user).map(str::to_owned),..Default::default()};
                output.lock().unwrap().commit_runtime_permissions(user as u32,&desired,inode)?;Ok(user as u32)
            });
            tx.send(result.as_ref().map(Clone::clone).map_err(|error|error.error.message.clone())).unwrap();result
        }).unwrap();
        metadata.lock().unwrap().set_version(10,7);
        assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap_err().contains("creation metadata is absent"));
        assert_eq!(metadata.lock().unwrap().pending_write_requests(),vec![10]);
        // Observe a real creator's file/stat and its recorded guest inode. Its
        // metadata differs from user0's constructor input, so copying that
        // frozen input cannot satisfy the write or the final inode assertion.
        let parent=data.0.join("misc_de/10/apexdata/com.android.permission");let probe=parent.join("original-creator-probe");
        let file=fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(&probe).unwrap();let stat=file.metadata().unwrap();
        let observed=GuestInode{uid:Some(stat.uid()),gid:Some(stat.gid()),mode:Some(stat.mode()&0o7777)};
        guest_inode::record(&probe,observed).unwrap();let observed=guest_inode::read(&probe).unwrap().unwrap();drop(file);fs::remove_file(probe).unwrap();
        assert_ne!(observed,initial[0].1);
        metadata.lock().unwrap().publish_user_creation_inode(10,observed).unwrap();
        // Handoff's wakeup must retry the same worker; no explicit retry(),
        // manual pump, replacement worker or constructor-map rewrite is used.
        assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap().unwrap(),vec![10]);
        metadata.lock().unwrap().bind_creation_metadata(&initial.into()).unwrap();
        assert_eq!(metadata.lock().unwrap().creation_inode(10),Some(observed));
        assert!(metadata.lock().unwrap().publish_user_creation_inode(10,observed).is_err());
        let path=parent.join("runtime-permissions.xml");assert_eq!(guest_inode::read(&path).unwrap(),Some(observed));
        assert_eq!(disk.lock().unwrap().state().users.iter().find(|(id,_)|*id==10).unwrap().1.runtime_permissions.as_ref().unwrap().version,7);
        assert!(metadata.lock().unwrap().pending_write_requests().is_empty());
        metadata.lock().unwrap().remove_user(10);assert!(metadata.lock().unwrap().creation_inode(10).is_none());drop(worker);
    }

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

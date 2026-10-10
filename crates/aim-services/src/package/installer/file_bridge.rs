//! Original FileBridge 8-byte big-endian stream protocol (android-16.0.0_r1).
//! AOSP, Apache License 2.0. This is selected only for ENABLE_REVOCABLE_FD=false.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
#[derive(Default)]
pub struct Transfer {
    closed: AtomicBool,
    stop: AtomicBool,
    server: Mutex<Option<UnixStream>>,
    proxy: Mutex<Option<Arc<aim_binder_host::proxy_file::Owner>>>,
    error: Mutex<Option<String>>,
}
impl Transfer {
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
            || self
                .proxy
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|owner| owner.is_revoked())
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(proxy) = self.proxy.lock().unwrap().as_ref() {
            proxy.revoke();
        }
        if let Some(server) = self.server.lock().unwrap().as_ref() {
            let _ = server.shutdown(Shutdown::Both);
        }
        self.closed.store(true, Ordering::SeqCst);
    }
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().take()
    }
    pub fn start(self: &Arc<Self>, mut target: File) -> io::Result<(UnixStream, JoinHandle<()>)> {
        let (mut server, client) = UnixStream::pair()?;
        {
            let mut retained = self.server.lock().unwrap();
            if self.stop.load(Ordering::SeqCst) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "installer write transfer stopped",
                ));
            }
            *retained = Some(server.try_clone()?);
        }
        let owner = self.clone();
        let worker = thread::Builder::new()
            .name("package-file-bridge".into())
            .spawn(move || {
                let run = (|| -> io::Result<()> {
                    let mut header = [0u8; 8];
                    let mut buffer = [0u8; 8192];
                    while !owner.stop.load(Ordering::SeqCst)
                        && server.read(&mut header)? == 8
                    {
                        let command = i32::from_be_bytes(header[..4].try_into().unwrap());
                        match command {
                            1 => {
                                let mut left = i32::from_be_bytes(header[4..].try_into().unwrap());
                                while left > 0 {
                                    let length = (left as usize).min(buffer.len());
                                    let n = server.read(&mut buffer[..length])?;
                                    if n == 0 {
                                        return Err(io::Error::new(
                                            io::ErrorKind::UnexpectedEof,
                                            format!("Unexpected EOF; still expected {left} bytes"),
                                        ));
                                    }
                                    target.write_all(&buffer[..n])?;
                                    left -= n as i32;
                                }
                            }
                            2 => {
                                target.sync_all()?;
                                server.write_all(&header)?;
                            }
                            3 => {
                                target.sync_all()?;
                                drop(target);
                                owner.closed.store(true, Ordering::SeqCst);
                                server.write_all(&header)?;
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })();
                if let Err(error) = run {
                    if !owner.stop.load(Ordering::SeqCst) {
                        *owner.error.lock().unwrap() = Some(error.to_string());
                    }
                }
                owner.server.lock().unwrap().take();
                owner.closed.store(true, Ordering::SeqCst);
            })?;
        Ok((client, worker))
    }
}
#[derive(Default)]
pub struct Registry {
    transfers: Mutex<BTreeMap<i32, Vec<Arc<Transfer>>>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    proxies: Mutex<Vec<aim_binder_host::proxy_file::Worker>>,
    stopped: AtomicBool,
    guard_taken: AtomicBool,
}
impl Registry {
    pub fn register(&self, id: i32) -> io::Result<Arc<Transfer>> {
        let mut transfers = self.transfers.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "installer write owner stopped",
            ));
        }
        let transfer = Arc::new(Transfer::default());
        transfers.entry(id).or_default().push(transfer.clone());
        Ok(transfer)
    }
    pub fn start(&self, id: i32, transfer: Arc<Transfer>, target: File) -> io::Result<UnixStream> {
        let mut workers = self.workers.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            transfer.stop();
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "installer write owner stopped",
            ));
        }
        let (client, worker) = transfer.start(target)?;
        workers.push(worker);
        if self.stopped.load(Ordering::SeqCst) {
            transfer.stop();
        }
        let _ = id;
        Ok(client)
    }
    pub fn start_proxy(
        &self,
        transfer: Arc<Transfer>,
        target: File,
    ) -> io::Result<aim_binder_driver::File> {
        let mut workers = self.proxies.lock().unwrap();
        if self.stopped.load(Ordering::SeqCst) {
            transfer.stop();
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "installer write owner stopped",
            ));
        }
        let (owner, client, worker) = aim_binder_host::proxy_file::open(target)?;
        use std::os::fd::AsFd;
        let capability = aim_binder_host::server::proxy_file_from_fd(client.as_fd())
            .ok_or_else(|| io::Error::other("Cannot retain proxy fileport"))?;
        *transfer.proxy.lock().unwrap() = Some(owner);
        workers.push(worker);
        if self.stopped.load(Ordering::SeqCst) {
            transfer.stop();
        }
        Ok(capability)
    }
    pub fn open(&self, id: i32) -> bool {
        self.transfers
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|transfers| transfers.iter().any(|transfer| !transfer.is_closed()))
    }
    pub fn stop_sessions(&self, ids: &[i32]) {
        let transfers = self.transfers.lock().unwrap();
        for id in ids {
            if let Some(transfers) = transfers.get(id) {
                for transfer in transfers {
                    transfer.stop();
                }
            }
        }
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        let transfers = self.transfers.lock().unwrap();
        for transfers in transfers.values() {
            for transfer in transfers {
                transfer.stop();
            }
        }
    }
    pub fn take_guard(self: &Arc<Self>) -> Option<IoWorkerGuard> {
        (!self.guard_taken.swap(true, Ordering::SeqCst)).then(|| IoWorkerGuard(self.clone()))
    }
    pub fn take_errors(&self) -> Vec<String> {
        self.transfers
            .lock()
            .unwrap()
            .values()
            .flat_map(|transfers| transfers.iter().filter_map(|transfer| transfer.error()))
            .collect()
    }
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop();
        for worker in self.proxies.get_mut().unwrap().drain(..) {
            worker.detach();
        }
    }
}
pub struct IoWorkerGuard(Arc<Registry>);
impl IoWorkerGuard {
    pub fn is_finished(&self) -> bool {
        self.0
            .workers
            .lock()
            .unwrap()
            .iter()
            .all(JoinHandle::is_finished)
            && self
                .0
                .proxies
                .lock()
                .unwrap()
                .iter()
                .all(aim_binder_host::proxy_file::Worker::is_finished)
    }
}
impl Drop for IoWorkerGuard {
    fn drop(&mut self) {
        self.0.stop();
        self.0.proxies.lock().unwrap().clear();
        for worker in self.0.workers.lock().unwrap().drain(..) {
            if worker.thread().id() != thread::current().id() {
                worker.join().unwrap();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Data(std::path::PathBuf);
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_file(&self.0).unwrap();
        }
    }
    fn target() -> (Data, File) {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "aim-filebridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&p)
            .unwrap();
        (Data(p), file)
    }
    fn command(client: &mut UnixStream, code: i32, length: i32) {
        let mut header = [0u8; 8];
        header[..4].copy_from_slice(&code.to_be_bytes());
        header[4..].copy_from_slice(&length.to_be_bytes());
        client.write_all(&header).unwrap();
    }
    #[test]
    fn session_stop_before_worker_start_rejects_the_transfer() {
        let (_data, file) = target();
        let registry = Arc::new(Registry::default());
        let guard = registry.take_guard().unwrap();
        let transfer = registry.register(7).unwrap();
        registry.stop_sessions(&[7]);
        let error = registry.start(7, transfer.clone(), file).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
        assert!(transfer.is_closed());
        assert!(guard.is_finished());
        drop(guard);
    }
    #[test]
    fn fileport_client_writes_syncs_closes_and_acknowledges_original_headers() {
        let (data, file) = target();
        let registry = Arc::new(Registry::default());
        let guard = registry.take_guard().unwrap();
        let transfer = registry.register(7).unwrap();
        let client = registry.start(7, transfer.clone(), file).unwrap();
        use std::os::fd::AsFd;
        let capability = aim_binder_host::server::file_from_fd(client.as_fd()).unwrap();
        drop(client);
        let fd = aim_binder_host::server::file_fd(&capability).unwrap();
        let mut client = UnixStream::from(fd.into_owned_fd().unwrap());
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        assert!(registry.open(7));
        command(&mut client, 1, 5);
        client.write_all(b"hello").unwrap();
        command(&mut client, 2, 5);
        let mut ack = [0u8; 8];
        client.read_exact(&mut ack).unwrap();
        assert_eq!(i32::from_be_bytes(ack[..4].try_into().unwrap()), 2);
        assert_eq!(std::fs::read(&data.0).unwrap(), b"hello");
        command(&mut client, 3, 5);
        client.read_exact(&mut ack).unwrap();
        assert_eq!(i32::from_be_bytes(ack[..4].try_into().unwrap()), 3);
        assert!(transfer.is_closed());
        drop(client);
        drop(guard);
        assert!(registry.take_errors().is_empty());
    }
    #[test]
    fn short_header_and_eof_close_owner_and_nonjoining_stop_wakes_waiter() {
        let (_data, file) = target();
        let registry = Arc::new(Registry::default());
        let guard = registry.take_guard().unwrap();
        let transfer = registry.register(8).unwrap();
        let mut client = registry.start(8, transfer.clone(), file).unwrap();
        client.write_all(&[0, 0, 0, 1]).unwrap();
        drop(client);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !transfer.is_closed() {
            assert!(std::time::Instant::now() < deadline);
            thread::sleep(std::time::Duration::from_millis(5));
        }
        let (_data2, file) = target();
        let transfer2 = registry.register(9).unwrap();
        let _client = registry.start(9, transfer2.clone(), file).unwrap();
        registry.stop();
        assert!(transfer2.is_closed());
        drop(guard);
    }
}

//! Native MoveCallbacks, ported from AOSP android-16.0.0_r1 (Apache-2.0).
use aim_binder_host::{local::{LocalProcess, LocalService, Strong}, parcel::{Binder, Exception, Parcel}};
use aim_service_aidl::android_content_pm_ipackagemoveobserver as observer;
use std::{collections::BTreeMap, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicI32, Ordering}}};

struct PrimaryObserver { owner: std::sync::Weak<Owner>, id: i32 }
impl aim_binder_host::local::Service for PrimaryObserver {
    fn descriptor(&self) -> &str { observer::DESCRIPTOR }
    fn transact(&self, call: &mut aim_binder_host::local::Call<'_>) -> aim_binder_host::local::Reply {
        match call.code {
            observer::ON_STATUS_CHANGED => {
                let args = observer::OnStatusChanged::read(&mut call.data)?;
                if call.data.remaining() != 0 { return Err(aim_binder_host::parcel::BAD_VALUE); }
                if let Some(owner) = self.owner.upgrade() {
                    if let Err(error) = owner.changed(self.id, args.status, args.est_millis) {
                        owner.errors.lock().unwrap().push(format!("primary move status: {error:?}"));
                    }
                }
            }
            observer::ON_CREATED => {
                call.data.enforce_interface(observer::DESCRIPTOR)?;
                call.data.read_i32()?;
                let present = call.data.read_i32()?;
                if present != 0 { crate::bundle::read(&mut call.data)?; }
                if call.data.remaining() != 0 { return Err(aim_binder_host::parcel::BAD_VALUE); }
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        Ok(Parcel::new())
    }
}

type Key = (bool, u64);
enum Target { Local(LocalService), Remote(Strong) }
impl Target {
    fn send(&self, code: u32, parcel: &Parcel) -> Result<(), i32> {
        match self {
            Self::Local(service) => service.transact(code, parcel, true).map(|_| ()),
            Self::Remote(service) => service.transact(code, parcel, true).map(|_| ()),
        }
    }
}
struct Listener { target: Target, death: Option<u64> }
pub struct Owner {
    process: Arc<LocalProcess>, events: Arc<super::events::Owner>,
    listeners: Mutex<BTreeMap<Key, Arc<Listener>>>, statuses: Mutex<BTreeMap<i32, i32>>,
    next: AtomicI32, closed: AtomicBool, errors: Mutex<Vec<String>>,
}
fn key(binder: Binder) -> Key {
    match binder { Binder::Local(pointer) => (false, pointer), Binder::Handle(handle) => (true, handle.into()) }
}
impl Owner {
    pub fn move_primary(self: &Arc<Self>, bridge: &Strong, volume: Option<String>) -> Result<i32, Exception> {
        use aim_service_aidl::dev_aim_server_ipackagemovebridge as api;
        let id = self.next_id()?;
        let mut extras = Parcel::new();
        crate::bundle::write(&mut extras, &[("android.os.storage.extra.FS_UUID", crate::bundle::Value::String(volume.clone()))]);
        self.created(id, &extras)?;
        let callback = self.process.add_service(Arc::new(PrimaryObserver { owner: Arc::downgrade(self), id }));
        let mut request = Parcel::new();
        api::MovePrimaryStorage { volume_uuid: volume, callback: Some(callback) }.write(&mut request);
        let reply = bridge.transact(api::MOVE_PRIMARY_STORAGE, &request, false).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("primary storage move: {status}")))?;
        let mut reader = reply.reader();
        api::read_move_primary_storage_reply(&mut reader).map_err(|status|
            Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("primary storage reply: {status}")))??;
        if reader.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, "trailing primary storage reply")); }
        Ok(id)
    }
    pub fn new(process: Arc<LocalProcess>, events: Arc<super::events::Owner>) -> Arc<Self> {
        Arc::new(Self { process, events, listeners: Mutex::new(BTreeMap::new()),
            statuses: Mutex::new(BTreeMap::new()), next: AtomicI32::new(0),
            closed: AtomicBool::new(false), errors: Mutex::new(Vec::new()) })
    }
    fn current(&self) -> Result<(), Exception> {
        if self.closed.load(Ordering::Acquire) { Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE, "package move owner closed")) } else { Ok(()) }
    }
    pub fn next_id(&self) -> Result<i32, Exception> {
        self.current()?; Ok(self.next.fetch_add(1, Ordering::Relaxed))
    }
    pub fn status(&self, id: i32) -> Result<i32, Exception> {
        self.current()?; Ok(*self.statuses.lock().unwrap().get(&id).unwrap_or(&0))
    }
    pub fn register(self: &Arc<Self>, binder: Option<Binder>) -> Result<(), Exception> {
        self.current()?;
        let binder = binder.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null move callback"))?;
        let identity = key(binder);
        let listener = match binder {
            Binder::Local(pointer) => Listener { target: Target::Local(self.process.local_service(pointer)
                .ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT, "unknown local callback"))?), death: None },
            Binder::Handle(handle) => {
                let target = self.process.strong(handle);
                let owner = Arc::downgrade(self);
                let death = self.process.link_to_death(&target, Box::new(move || {
                    if let Some(owner) = owner.upgrade() {
                        let removed = owner.listeners.lock().unwrap().remove(&identity);
                        drop(removed);
                    }
                }));
                Listener { target: Target::Remote(target), death: Some(death) }
            }
        };
        let replaced = self.listeners.lock().unwrap().insert(identity, Arc::new(listener));
        if let Some(old) = replaced { self.release(&old); }
        Ok(())
    }
    fn release(&self, listener: &Listener) {
        if let (Target::Remote(target), Some(death)) = (&listener.target, listener.death) {
            self.process.clear_death(target, death);
        }
    }
    pub fn unregister(&self, binder: Option<Binder>) -> Result<(), Exception> {
        self.current()?;
        let binder = binder.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null move callback"))?;
        let removed = self.listeners.lock().unwrap().remove(&key(binder));
        if let Some(listener) = removed { self.release(&listener); }
        Ok(())
    }
    fn enqueue(self: &Arc<Self>, code: u32, parcel: Parcel) -> Result<(), Exception> {
        self.current()?;
        let owner = self.clone();
        self.events.post(false, Box::new(move || {
            if owner.closed.load(Ordering::Acquire) { return; }
            let targets: Vec<_> = owner.listeners.lock().unwrap().values().cloned().collect();
            for target in targets {
                if let Err(status) = target.target.send(code, &parcel) {
                    owner.errors.lock().unwrap().push(format!("move observer transport: {status}"));
                }
            }
        }))
    }
    pub fn created(self: &Arc<Self>, id: i32, extras: &Parcel) -> Result<(), Exception> {
        let mut parcel = Parcel::new(); parcel.write_interface_token(observer::DESCRIPTOR);
        parcel.write_i32(id); parcel.write_i32(1);
        parcel.write_raw(extras.data(), extras.objects());
        self.enqueue(observer::ON_CREATED, parcel)
    }
    pub fn changed(self: &Arc<Self>, id: i32, status: i32, estimate: i64) -> Result<(), Exception> {
        let mut parcel = Parcel::new(); parcel.write_interface_token(observer::DESCRIPTOR);
        parcel.write_i32(id); parcel.write_i32(status); parcel.write_i64(estimate);
        self.enqueue(observer::ON_STATUS_CHANGED, parcel)?;
        self.statuses.lock().unwrap().insert(id, status);
        Ok(())
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        let listeners = std::mem::take(&mut *self.listeners.lock().unwrap());
        for (_, listener) in listeners { self.release(&listener); }
    }
}

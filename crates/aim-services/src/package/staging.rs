//! Native StagingManager observer/info ownership. android-16.0.0_r1, AOSP Apache-2.0.
use aim_binder_host::{
    local::{LocalProcess, Strong},
    parcel::{BAD_VALUE, Binder, Exception, Parcel, Reader},
};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApexInfo {
    pub module_name: Option<String>,
    pub disk_image_path: Option<String>,
    pub version_code: i64,
    pub version_name: Option<String>,
    pub has_class_path_jars: bool,
}
impl WriteParcelable for ApexInfo {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_string16(self.module_name.as_deref());
        p.write_string16(self.disk_image_path.as_deref());
        p.write_i64(self.version_code);
        p.write_string16(self.version_name.as_deref());
        p.write_bool(self.has_class_path_jars);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}
impl ReadParcelable for ApexInfo {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        let end = start.checked_add(size as usize).ok_or(BAD_VALUE)?;
        if end > r.position() + r.remaining() {
            return Err(BAD_VALUE);
        }
        let mut value = Self::default();
        if r.position() < end {
            value.module_name = r.read_string16()?;
        }
        if r.position() < end {
            value.disk_image_path = r.read_string16()?;
        }
        if r.position() < end {
            value.version_code = r.read_i64()?;
        }
        if r.position() < end {
            value.version_name = r.read_string16()?;
        }
        if r.position() < end {
            value.has_class_path_jars = r.read_bool()?;
        }
        if r.position() > end {
            return Err(BAD_VALUE);
        }
        r.set_position(end);
        Ok(value)
    }
}
struct Event(Vec<Option<ApexInfo>>);
impl WriteParcelable for Event {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        aim_service_aidl::write_typed_list(p, Some(&self.0));
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}
/// Supplied only after the native installer has received successful staged
/// verification from apexd. Child ids contain APEX sessions only.
#[derive(Clone, Debug)]
pub struct ReadySession {
    pub id: i32,
    pub apex_children: Vec<i32>,
}
struct Observer {
    node: Arc<Strong>,
    death: u64,
}
pub struct Owner {
    process: Arc<LocalProcess>,
    bridge: Strong,
    closed: AtomicBool,
    ready: Mutex<BTreeMap<i32, ReadySession>>,
    observers: Mutex<Vec<Observer>>,
}
impl Owner {
    pub fn new(process: Arc<LocalProcess>, bridge: Strong) -> Arc<Self> {
        Arc::new(Self {
            process,
            bridge,
            closed: AtomicBool::new(false),
            ready: Mutex::new(BTreeMap::new()),
            observers: Mutex::new(Vec::new()),
        })
    }
    pub fn register(self: &Arc<Self>, observer: Option<Binder>) -> Result<(), Exception> {
        self.ensure_open()?;
        let Some(observer) = observer else {
            return Ok(());
        };
        let Binder::Handle(handle) = observer else {
            return Err(Exception::illegal_argument(
                "staged observer must be a received remote capability",
            ));
        };
        let node = Arc::new(self.process.strong(handle));
        let owner: Weak<Self> = Arc::downgrade(self);
        let death = self.process.link_to_death(
            &node,
            Box::new(move || {
                if let Some(owner) = owner.upgrade() {
                    owner
                        .observers
                        .lock()
                        .unwrap()
                        .retain(|item| item.node.handle != handle);
                }
            }),
        );
        let mut observers = self.observers.lock().unwrap();
        if self.closed.load(Ordering::Acquire) {
            drop(observers);
            self.process.clear_death(&node, death);
            return Err(self.closed_error());
        }
        observers.push(Observer { node, death });
        Ok(())
    }
    pub fn unregister(&self, observer: Option<Binder>) -> Result<(), Exception> {
        self.ensure_open()?;
        let Some(Binder::Handle(handle)) = observer else {
            return Ok(());
        };
        let removed = {
            let mut observers = self.observers.lock().unwrap();
            observers
                .iter()
                .position(|item| item.node.handle == handle)
                .map(|index| observers.remove(index))
        };
        if let Some(item) = removed {
            self.process.clear_death(&item.node, item.death);
        }
        Ok(())
    }
    pub fn infos(&self) -> Result<Vec<Option<ApexInfo>>, Exception> {
        self.ensure_open()?;
        use aim_service_aidl::dev_aim_server_inativestagingbridge as aidl;
        let ready = self
            .ready
            .lock()
            .unwrap()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut infos = Vec::new();
        for session in ready {
            let mut request = Parcel::new();
            aidl::GetStagedApexInfos {
                session_id: session.id,
                apex_child_session_ids: Some(session.apex_children),
            }
            .write(&mut request);
            let reply = self
                .bridge
                .transact(aidl::GET_STAGED_APEX_INFOS, &request, false)
                .map_err(|status| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("apexd transport {status}"),
                    )
                })?;
            let mut r = reply.reader();
            let rows = aidl::read_get_staged_apex_infos_reply::<ApexInfo>(&mut r).map_err(
                |status| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("apexd reply {status}"),
                    )
                },
            )??;
            if r.remaining() != 0 {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "apexd reply has trailing bytes",
                ));
            }
            infos.extend(rows.ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "apexd returned null staged infos",
                )
            })?);
        }
        self.ensure_open()?;
        Ok(infos)
    }
    /// Native readiness and abandonment call this after updating installer
    /// persistence. Notifications use the resulting live owner, never boot APEX inventory.
    pub fn publish_ready(&self, session: ReadySession) -> Result<(), Exception> {
        self.ensure_open()?;
        self.ready.lock().unwrap().insert(session.id, session);
        self.notify()
    }
    pub fn remove(&self, id: i32) -> Result<(), Exception> {
        self.ensure_open()?;
        self.ready.lock().unwrap().remove(&id);
        self.notify()
    }
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let observers = std::mem::take(&mut *self.observers.lock().unwrap());
        self.ready.lock().unwrap().clear();
        for observer in observers {
            self.process.clear_death(&observer.node, observer.death);
        }
    }
    fn closed_error(&self) -> Exception {
        Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            "native staging owner closed",
        )
    }
    fn ensure_open(&self) -> Result<(), Exception> {
        if self.closed.load(Ordering::Acquire) {
            Err(self.closed_error())
        } else {
            Ok(())
        }
    }
    fn notify(&self) -> Result<(), Exception> {
        use aim_service_aidl::android_content_pm_istagedapexobserver as aidl;
        let event = Event(self.infos()?);
        let mut request = Parcel::new();
        aidl::OnApexStaged { event: Some(event) }.write(&mut request);
        // The original treats remote observer failure as a warning, preserving readiness.
        let observers = self
            .observers
            .lock()
            .unwrap()
            .iter()
            .map(|observer| observer.node.clone())
            .collect::<Vec<_>>();
        for observer in observers.iter() {
            if let Err(status) = observer.transact(aidl::ON_APEX_STAGED, &request, true) {
                eprintln!("staged APEX observer transport failed: {status}");
            }
        }
        Ok(())
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        for observer in self.observers.get_mut().unwrap().drain(..) {
            self.process.clear_death(&observer.node, observer.death);
        }
    }
}

/// Dispatch the three IPackageManagerNative staging methods against this
/// owner; neither boot APEX enumeration nor original PMS answers these calls.
pub fn transact(
    owner: &Arc<Owner>,
    call: &mut aim_binder_host::local::Call<'_>,
) -> Option<aim_binder_host::local::Reply> {
    use aim_service_aidl::android_content_pm_ipackagemanagernative as aidl;
    enum Action {
        Register(Option<Binder>),
        Unregister(Option<Binder>),
        Infos,
    }
    let parsed = match call.code {
        aidl::REGISTER_STAGED_APEX_OBSERVER => {
            aidl::RegisterStagedApexObserver::read(&mut call.data)
                .map(|args| Action::Register(args.observer))
        }
        aidl::UNREGISTER_STAGED_APEX_OBSERVER => {
            aidl::UnregisterStagedApexObserver::read(&mut call.data)
                .map(|args| Action::Unregister(args.observer))
        }
        aidl::GET_STAGED_APEX_INFOS => {
            aidl::GetStagedApexInfos::read(&mut call.data).map(|_| Action::Infos)
        }
        _ => return None,
    };
    Some((|| {
        let action = parsed?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        let result = match action {
            Action::Register(observer) => owner
                .register(observer)
                .map(|()| aidl::write_register_staged_apex_observer_reply(&mut reply)),
            Action::Unregister(observer) => owner
                .unregister(observer)
                .map(|()| aidl::write_unregister_staged_apex_observer_reply(&mut reply)),
            Action::Infos => owner
                .infos()
                .map(|infos| aidl::write_get_staged_apex_infos_reply(&mut reply, Some(&infos))),
        };
        if let Err(error) = result {
            reply.write_exception(&error);
        }
        Ok(reply)
    })())
}

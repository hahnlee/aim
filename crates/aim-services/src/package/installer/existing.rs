//! InstallPackageHelper.installExistingPackageAsUser, android-16.0.0_r1 (AOSP).
//! Existing code is retained; native Settings owns the user install transition.
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

#[derive(Clone, Debug)]
pub struct IntentSender(pub Option<Binder>);
impl ReadParcelable for IntentSender {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self, i32> {
        Ok(Self(reader.read_binder()?))
    }
}
impl WriteParcelable for IntentSender {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_binder(self.0);
    }
}
#[derive(Clone, Debug)]
pub struct Request {
    pub package: Option<String>,
    pub user: i32,
    pub flags: i32,
    pub reason: i32,
    pub allowlisted_permissions: Option<Vec<Option<String>>>,
    pub receiver: Option<IntentSender>,
}
pub type Completion = Box<dyn FnOnce(bool) -> Result<(), Exception> + Send>;
struct Pending {
    entries: BTreeMap<i32, Completion>,
    stopped: bool,
}
pub struct Restores(Mutex<Pending>);
impl Default for Restores {
    fn default() -> Self {
        Self(Mutex::new(Pending {
            entries: BTreeMap::new(),
            stopped: false,
        }))
    }
}
impl Restores {
    pub fn register(&self, completion: Completion) -> Result<i32, Exception> {
        let mut pending = self.0.lock().unwrap();
        if pending.stopped {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer restore owner stopped",
            ));
        }
        // Service endpoints survive bootstrap replacement. A late original
        // BackupManager callback must never consume a new owner's reused token.
        static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
        let token = loop {
            let current = NEXT.load(std::sync::atomic::Ordering::Relaxed);
            let token = if current < 0 { 1 } else { current };
            if NEXT
                .compare_exchange_weak(
                    current,
                    token.wrapping_add(1),
                    std::sync::atomic::Ordering::Relaxed,
                    std::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                break token;
            }
        };
        pending.entries.insert(token, completion);
        Ok(token)
    }
    pub fn cancel(&self, token: i32) -> bool {
        self.0.lock().unwrap().entries.remove(&token).is_some()
    }
    pub fn finish(&self, token: i32, did_launch: bool) -> Result<bool, Exception> {
        let completion = self.0.lock().unwrap().entries.remove(&token);
        match completion {
            Some(completion) => {
                completion(did_launch)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
    pub fn shutdown(&self) {
        let mut pending = self.0.lock().unwrap();
        pending.stopped = true;
        pending.entries.clear();
    }
}
pub struct Owner {
    pub system: Weak<crate::system::System>,
    pub bridge: Arc<crate::package::bootstrap::Bridge>,
    pub restores: Arc<Restores>,
    pub effects: Arc<crate::package::effects::Owner>,
    pub changes: Arc<crate::package::changes::Owner>,
    errors: Arc<Mutex<Vec<String>>>,
}
impl Owner {
    pub fn new(
        system: Weak<crate::system::System>,
        bridge: Arc<crate::package::bootstrap::Bridge>,
        effects: Arc<crate::package::effects::Owner>,
        changes: Arc<crate::package::changes::Owner>,
        restores: Arc<Restores>,
    ) -> Arc<Self> {
        Arc::new(Self {
            system,
            bridge,
            effects,
            changes,
            restores,
            errors: Arc::new(Mutex::new(Vec::new())),
        })
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
    pub fn complete_immediate(&self, request: Request, status: i32) -> Result<i32, Exception> {
        if let Some(diagnostic) = self
            .bridge
            .complete_existing_install(
                request.package.as_deref(),
                request.user,
                request.receiver.as_ref(),
                status,
                false,
            )
            .map_err(|error| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("existing install completion: {error:?}"),
                )
            })?
        {
            self.errors.lock().unwrap().push(diagnostic);
        }
        Ok(status)
    }
    pub fn start_restore(&self, request: Request, package: &str) -> Result<(), Exception> {
        let system = self
            .system
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "existing restore system stopped"))?;
        let guard = system.retain_existing_status_receiver(request.receiver.as_ref())?;
        let bridge = self.bridge.clone();
        let weak = self.system.clone();
        let name = package.to_owned();
        let errors = self.errors.clone();
        let completion_name = name.clone();
        let receiver = request.receiver;
        let user = request.user;
        let token = self.restores.register(Box::new(move |_| {
            let _guard = guard;
            let system = weak.upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "existing restore system stopped")
            })?;
            system.check_package_bootstrap(&bridge)?;
            bridge
                .complete_existing_install(Some(&completion_name), user, None, 1, true)
                .map_err(|error| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("existing permission restore: {error:?}"),
                    )
                })?;
            system.restore_existing_install_preferences(&completion_name, user)?;
            if let Some(diagnostic) = bridge
                .complete_existing_install(
                    Some(&completion_name),
                    user,
                    receiver.as_ref(),
                    1,
                    false,
                )
                .map_err(|error| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("existing restore completion: {error:?}"),
                    )
                })?
            {
                errors.lock().unwrap().push(diagnostic);
            }
            Ok(())
        }))?;
        match self.bridge.restore_existing_install(&name, user, token) {
            Ok(true) => Ok(()),
            Ok(false) => self.restores.finish(token, false).map(|_| ()),
            Err(error) => {
                self.errors.lock().unwrap().push(format!(
                    "existing backup restore failed; completing without restore: {error:?}"
                ));
                self.restores.finish(token, false).map(|_| ())
            }
        }
    }
    pub fn install_existing(&self, uid: u32, pid: i32, request: Request) -> Result<i32, Exception> {
        let system = self
            .system
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "existing install system stopped"))?;
        system.install_existing_package(self, uid as i32, pid, request)
    }
}

/// Synchronous bridge inputs retain their own bytes, independently of later publications.
pub struct Record {
    bytes: Mutex<Option<Vec<u8>>>,
}
impl Record {
    pub fn new(bytes: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            bytes: Mutex::new(Some(bytes)),
        })
    }
    pub fn revoke(&self) {
        self.bytes.lock().unwrap().take();
    }
}
impl aim_binder_host::local::Service for Record {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_iexistinginstallrecord::DESCRIPTOR
    }
    fn transact(
        &self,
        call: &mut aim_binder_host::local::Call<'_>,
    ) -> aim_binder_host::local::Reply {
        use aim_service_aidl::dev_aim_server_iexistinginstallrecord as api;
        enum Action {
            Length,
            Chunk(i32, i32),
            Close,
        }
        let action = match call.code {
            api::GET_LENGTH => {
                api::GetLength::read(&mut call.data)?;
                Action::Length
            }
            api::GET_CHUNK => {
                let args = api::GetChunk::read(&mut call.data)?;
                Action::Chunk(args.offset, args.length)
            }
            api::CLOSE => {
                api::Close::read(&mut call.data)?;
                Action::Close
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        if call.sender_euid != 0 && call.sender_euid != 1000 {
            reply.write_exception(&Exception::security(
                "existing install inputs require system UID",
            ));
            return Ok(reply);
        }
        if matches!(action, Action::Close) {
            self.revoke();
            api::write_close_reply(&mut reply);
            return Ok(reply);
        }
        let bytes = self.bytes.lock().unwrap();
        let Some(bytes) = bytes.as_ref() else {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "existing install inputs revoked",
            ));
            return Ok(reply);
        };
        match action {
            Action::Length => match i32::try_from(bytes.len()) {
                Ok(length) => api::write_get_length_reply(&mut reply, length),
                Err(_) => reply.write_exception(&Exception::new(
                    EX_ILLEGAL_STATE,
                    "existing install inputs exceed length ABI",
                )),
            },
            Action::Chunk(offset, length) => {
                let range = usize::try_from(offset)
                    .ok()
                    .zip(usize::try_from(length).ok())
                    .and_then(|(offset, length)| {
                        offset
                            .checked_add(length)
                            .filter(|end| *end <= bytes.len() && length <= 131072)
                            .map(|end| offset..end)
                    });
                if let Some(range) = range {
                    api::write_get_chunk_reply(&mut reply, &Some(bytes[range].to_vec()));
                } else {
                    reply.write_exception(&Exception::illegal_argument(
                        "invalid existing install input range",
                    ));
                }
            }
            Action::Close => unreachable!(),
        }
        Ok(reply)
    }
}

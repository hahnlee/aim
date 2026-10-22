//! `bugreport`: BugreportManagerServiceImpl for the app host
//! (`android-16.0.0_r1`): the original's checks (the caller's package, the
//! SystemConfig allowlist or the automotive projection role with DUMP, an
//! admin user), and the reports taken by dumpstate, which init starts as
//! `bugreportd` (`ctl.start`) and which registers `dumpstate`; its
//! listener's calls are passed on, and the files reported for a deferred
//! consent kept for their caller in `/data/system/bugreport-mapping.xml`
//! with `onboarding_bugreport_v2_enabled`. With no telephony no package
//! has carrier privileges. Dump and the shell command are #1943.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aim_android_xml::{Element, Node, Value as XmlValue};
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, Exception, Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_os_idumpstate as ds;
use aim_service_aidl::android_os_idumpstatelistener as dl;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::{AppHost, PER_USER_RANGE, reply};

/// `BugreportParams.BUGREPORT_MODE_*` the service takes; `DEFAULT` is not
/// one.
const MODES: [i32; 7] = [0, 1, 2, 3, 4, 5, 7];
const BUGREPORT_MODE_REMOTE: i32 = 2;
const BUGREPORT_MODE_TELEPHONY: i32 = 4;
/// `BugreportParams.BUGREPORT_FLAG_*`.
const FLAG_USE_PREDUMPED_UI_DATA: i32 = 0x1;
const FLAG_DEFER_CONSENT: i32 = 0x2;
const FLAG_KEEP_BUGREPORT_ON_RETRIEVAL: i32 = 0x4;
/// `IDumpstateListener.BUGREPORT_ERROR_*`.
const ERROR_RUNTIME_ERROR: i32 = 2;
const ERROR_ANOTHER_REPORT_IN_PROGRESS: i32 = 5;
const ERROR_NO_BUGREPORT_TO_RETRIEVE: i32 = 6;
const ROLE_SYSTEM_AUTOMOTIVE_PROJECTION: &str = "android.app.role.SYSTEM_AUTOMOTIVE_PROJECTION";
const DUMP: &str = "android.permission.DUMP";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// init's service, and the binder it registers.
const BUGREPORT_SERVICE: &str = "bugreportd";
const DUMPSTATE: &str = "dumpstate";
const DEFAULT_BUGREPORT_SERVICE_TIMEOUT: Duration = Duration::from_secs(30);
const CONSENTLESS_GRACE_PERIOD_MS: i64 = 2 * 60 * 1000;
/// The aconfig flags the service reads.
const ONBOARDING_BUGREPORT_V2: &str = "android.app.admin.flags.onboarding_bugreport_v2_enabled";
const ONBOARDING_CONSENTLESS: &str = "android.app.admin.flags.onboarding_consentless_bugreports";
const ASYNC_START_BUGREPORT: &str = "com.android.server.os.async_start_bugreport";
const MAPPING_FILE: &str = "system/bugreport-mapping.xml";
const TAG_BUGREPORT_DATA: &str = "bugreport-data";
const TAG_BUGREPORT_MAP: &str = "bugreport-map";
const TAG_PERSISTENT_BUGREPORT: &str = "persistent-bugreport";
const ATTR_CALLING_UID: &str = "calling-uid";
const ATTR_CALLING_PACKAGE: &str = "calling-package";
const ATTR_BUGREPORT_FILE: &str = "bugreport-file";
const SYSTEM_UID: i32 = 1000;
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");

type Result<T> = std::result::Result<T, Exception>;

/// `Objects.requireNonNull`'s exception.
fn null() -> Exception {
    Exception::new(EX_NULL_POINTER, "")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A `FileDescriptor` argument, as received.
struct ReceivedFd(u32);

impl ReadParcelable for ReceivedFd {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        r.read_fd().map(ReceivedFd)
    }
}

/// A `FileDescriptor` passed on.
struct SentFd(aim_binder_driver::File);

impl WriteParcelable for SentFd {
    fn write_to(&self, p: &mut Parcel) {
        p.write_file(self.0.clone());
    }
}

/// A caller: its uid and package (`Pair<Integer, String>`).
type Caller = (i32, Option<String>);

/// `Pair.hashCode`, the order of the mapping's `ArrayMap`.
fn caller_hash((uid, pkg): &Caller) -> i32 {
    uid ^ pkg.as_deref().map_or(0, crate::package::info::java_hash)
}

/// `BugreportFileManager`: the files reported for each caller, those kept
/// after retrieval, and the consents given.
#[derive(Default)]
struct Files {
    read: bool,
    /// As added; written in their map's order.
    files: Vec<(Caller, Vec<String>)>,
    to_persist: Vec<String>,
    /// By package: when the consent was given, and whether it covers a
    /// full report.
    consents: Vec<(String, (i64, bool))>,
}

impl Files {
    fn add_mapping(&mut self, caller: Caller, file: String) {
        match self.files.iter_mut().find(|(c, _)| *c == caller) {
            Some((_, files)) => {
                if !files.contains(&file) {
                    files.push(file);
                }
            }
            None => self.files.push((caller, vec![file])),
        }
    }

    /// `readBugreportMappingLocked`: the mapping read again; a file that
    /// cannot be is deleted.
    fn read_mapping(&mut self, path: &Path) {
        self.files.clear();
        let bytes = match read_atomic(path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                eprintln!("app host: bugreport: Bugreport mapping file does not exist");
                return;
            }
            Err(e) => {
                eprintln!("app host: bugreport: {}: {e}", path.display());
                return;
            }
        };
        match self.read_document(&bytes) {
            Ok(()) => self.read = true,
            Err(e) => {
                eprintln!("app host: bugreport: {}: {e}", path.display());
                delete_atomic(path);
            }
        }
    }

    fn read_document(&mut self, bytes: &[u8]) -> std::result::Result<(), String> {
        let root = aim_android_xml::read(bytes)?;
        if root.name != TAG_BUGREPORT_DATA {
            return Err(format!(
                "Unexpected start tag: found {}, expected {TAG_BUGREPORT_DATA}",
                root.name
            ));
        }
        for e in root.children() {
            let file = || e.string(ATTR_BUGREPORT_FILE).map(|f| f.into_owned());
            match e.name.as_str() {
                TAG_BUGREPORT_MAP => {
                    let uid = e
                        .int(ATTR_CALLING_UID)?
                        .ok_or_else(|| format!("missing {ATTR_CALLING_UID}"))?;
                    let pkg = e.string(ATTR_CALLING_PACKAGE).map(|p| p.into_owned());
                    if let Some(file) = file() {
                        self.add_mapping((uid, pkg), file);
                    }
                }
                TAG_PERSISTENT_BUGREPORT => {
                    if let Some(file) = file()
                        && !self.to_persist.contains(&file)
                    {
                        self.to_persist.push(file);
                    }
                }
                tag => eprintln!(
                    "app host: bugreport: Unknown tag while reading bugreport mapping file: {tag}"
                ),
            }
        }
        Ok(())
    }

    /// `writeBugreportDataLocked`, as TypedXmlSerializer writes it.
    fn write_mapping(&self, path: &Path) {
        if self.files.is_empty() && self.to_persist.is_empty() {
            return;
        }
        let element = |name: &str, attrs: Vec<(String, XmlValue)>| {
            Node::Element(Element {
                name: name.into(),
                attrs,
                content: Vec::new(),
            })
        };
        let mut callers: Vec<_> = self.files.iter().collect();
        callers.sort_by_key(|(c, _)| caller_hash(c));
        let mut content = Vec::new();
        for ((uid, pkg), files) in callers {
            let mut files: Vec<_> = files.iter().collect();
            files.sort_by_key(|f| crate::package::info::java_hash(f));
            for file in files {
                let mut attrs = vec![(ATTR_CALLING_UID.to_string(), XmlValue::Int(*uid))];
                if let Some(pkg) = pkg {
                    attrs.push((ATTR_CALLING_PACKAGE.into(), XmlValue::String(pkg.clone())));
                }
                attrs.push((ATTR_BUGREPORT_FILE.into(), XmlValue::String(file.clone())));
                content.push(element(TAG_BUGREPORT_MAP, attrs));
            }
        }
        for file in &self.to_persist {
            content.push(element(
                TAG_PERSISTENT_BUGREPORT,
                vec![(ATTR_BUGREPORT_FILE.into(), XmlValue::String(file.clone()))],
            ));
        }
        let root = Element {
            name: TAG_BUGREPORT_DATA.into(),
            attrs: Vec::new(),
            content,
        };
        if let Err(e) = write_atomic(path, &root) {
            eprintln!("app host: bugreport: Failed to write bugreport mapping file: {e}");
        }
    }

    /// `canSkipConsentScreen`.
    fn can_skip_consent(&mut self, consentless: bool, pkg: &str, full: bool) -> bool {
        if !consentless {
            return false;
        }
        let Some(at) = self.consents.iter().position(|(p, _)| p == pkg) else {
            return false;
        };
        let (granted, full_ok) = self.consents[at].1;
        if granted + CONSENTLESS_GRACE_PERIOD_MS < now_ms() {
            self.consents.remove(at);
            return false;
        }
        !(full && !full_ok)
    }

    /// `logConsentGrantedForCaller`.
    fn log_consent(&mut self, consentless: bool, pkg: &str, granted: bool, deferred: bool) {
        if !consentless {
            return;
        }
        let at = self.consents.iter().position(|(p, _)| p == pkg);
        if granted {
            let entry = (now_ms(), deferred);
            match at {
                Some(at) => self.consents[at].1 = entry,
                None => self.consents.push((pkg.to_string(), entry)),
            }
        } else if !deferred {
            match at {
                Some(at) => self.consents[at].1.1 = false,
                None => eprintln!(
                    "app host: bugreport: Previous consent from package: {pkg} shouldhave been logged."
                ),
            }
        }
    }
}

/// The bytes of `path` as `AtomicFile.openRead` reads them; none without
/// the file.
fn read_atomic(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let backup = with(".bak");
    if backup.exists() {
        std::fs::rename(&backup, path)?;
    }
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// `AtomicFile.delete`.
fn delete_atomic(path: &Path) {
    for suffix in ["", ".new", ".bak"] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(name));
    }
}

/// `AtomicFile.startWrite` and `finishWrite`: the system's, mode 0600.
fn write_atomic(path: &Path, root: &Element) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = aim_android_xml::abx::write(root).map_err(std::io::Error::other)?;
    let mut new = path.as_os_str().to_owned();
    new.push(".new");
    let new = PathBuf::from(new);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

/// `mLock`'s state.
#[derive(Default)]
struct State {
    pre_dumped_uid: Option<i32>,
    /// The listener of the report taking place (`mCurrentDumpstateListener`).
    current: Option<u64>,
}

pub struct Bugreport {
    host: Arc<AppHost>,
    state: Mutex<State>,
    files: Mutex<Files>,
    next_listener: AtomicU64,
    me: Weak<Bugreport>,
}

impl Bugreport {
    pub fn new(host: Arc<AppHost>) -> Arc<Bugreport> {
        Arc::new_cyclic(|me| Bugreport {
            host,
            state: Mutex::new(State::default()),
            files: Mutex::new(Files::default()),
            next_listener: AtomicU64::new(1),
            me: me.clone(),
        })
    }

    fn flag(&self, name: &str) -> Result<bool> {
        let (state, _) = self.host.packages()?;
        Ok(state.system.flags.iter().any(|(n, on)| n == name && *on))
    }

    /// `onboardingConsentlessBugreports()` on a debuggable build.
    fn consentless(&self) -> Result<bool> {
        Ok(self.flag(ONBOARDING_CONSENTLESS)?
            && self.host.property("ro.debuggable").as_deref() == Some("1"))
    }

    fn mapping_file(&self) -> Result<PathBuf> {
        Ok(self.host.data()?.join(MAPPING_FILE))
    }

    /// `enforcePermission`: the caller's package, allowlisted (by
    /// SystemConfig or the automotive projection role) and holding DUMP.
    /// With no telephony, no package has carrier privileges.
    fn enforce(&self, pkg: Option<&str>, uid: i32, carrier: bool) -> Result<()> {
        let owns = match pkg {
            Some(p) => self.host.owns_package(uid, p)?,
            None => false,
        };
        let pkg = pkg.unwrap_or("null");
        if !owns {
            return Err(Exception::security(format!(
                "Package {pkg} does not belong to {uid}"
            )));
        }
        let (state, _) = self.host.packages()?;
        let mut allowlisted = state.system.bugreport_allowlist.contains(pkg);
        if !allowlisted && let Some(roles) = self.host.role_service.get() {
            allowlisted = roles
                .holders_of(ROLE_SYSTEM_AUTOMOTIVE_PROJECTION)?
                .iter()
                .any(|h| h == pkg);
        }
        if allowlisted && self.host.check_permission(Some(DUMP), uid)? == 0 {
            return Ok(());
        }
        let message = format!(
            "{pkg} does not hold the DUMP permission or is not bugreport-whitelisted or does not have an allowed role {}to request a bugreport",
            if carrier {
                "and does not have carrier privileges "
            } else {
                ""
            }
        );
        eprintln!("app host: bugreport: {message}");
        Err(Exception::security(message))
    }

    /// `ensureUserCanTakeBugReport`: an admin user (the app host's users
    /// are no profiles); a remote report of a user affiliated with a
    /// device owner, of which there is none without admins.
    fn ensure_user_can_take(&self, uid: i32, mode: i32) -> Result<()> {
        let user = uid / PER_USER_RANGE as i32;
        if super::user_service::is_admin(&self.host, user)? {
            return Ok(());
        }
        if mode == BUGREPORT_MODE_REMOTE && self.host.device_policies()?.has_admins_or_policies() {
            return Err(self.host.unimplemented_named(
                "bugreport",
                "a user affiliated with a device owner (#1521)",
            ));
        }
        let message = format!(
            "Calling user {user} is not an admin user. Only admin users and their profiles are allowed to take bugreport."
        );
        eprintln!("app host: bugreport: {message}");
        Err(Exception::illegal_argument(message))
    }

    /// `getDumpstateBinderServiceLocked`.
    fn dumpstate(&self) -> Option<Arc<Strong>> {
        super::display::check_service(&self.host.process, DUMPSTATE)
            .ok()
            .map(Arc::new)
    }

    /// `startAndGetDumpstateBinderServiceLocked`: `bugreportd` started, its
    /// binder waited for with a doubling wait, up to 30 s.
    fn start_dumpstate(&self) -> Option<Arc<Strong>> {
        self.host.set_property("ctl.start", BUGREPORT_SERVICE);
        let mut waited = Duration::ZERO;
        let mut wait = Duration::from_millis(500);
        loop {
            if let Some(ds) = self.dumpstate() {
                eprintln!("app host: bugreport: Got bugreport service handle.");
                return Some(ds);
            }
            std::thread::sleep(wait);
            eprintln!(
                "app host: bugreport: Waiting to get dumpstate service handle ({}ms)",
                waited.as_millis()
            );
            waited += wait;
            wait *= 2;
            if waited > DEFAULT_BUGREPORT_SERVICE_TIMEOUT {
                eprintln!(
                    "app host: bugreport: Timed out waiting to get dumpstate service handle ({}ms)",
                    waited.as_millis()
                );
                return None;
            }
        }
    }

    /// `stopDumpstateBinderServiceLocked`.
    fn stop_dumpstate(&self) {
        self.host.set_property("ctl.stop", BUGREPORT_SERVICE);
    }

    /// A call to dumpstate: the exception it threw, none when it could not
    /// be made or its reply read (a `RemoteException`).
    fn call(dumpstate: &Strong, code: u32, data: &Parcel) -> Option<Result<()>> {
        let reply = dumpstate.transact(code, data, false).ok()?;
        reply.reader().read_exception().ok()
    }

    /// `reportError`: a failure to call is logged.
    fn report_error(listener: &Strong, code: i32) {
        let mut data = Parcel::new();
        dl::OnError { error_code: code }.write(&mut data);
        if let Err(status) = listener.transact(dl::ON_ERROR, &data, true) {
            eprintln!("app host: bugreport: onError() transaction failed: {status}");
        }
    }

    fn file(&self, fd: ReceivedFd) -> Result<SentFd> {
        self.host
            .process
            .file(fd.0)
            .map(SentFd)
            .ok_or_else(|| Exception::illegal_argument("bad file descriptor"))
    }

    /// `preDumpUiData`.
    fn pre_dump(&self, pkg: Option<&str>, uid: i32) -> Result<()> {
        self.enforce(pkg, uid, true)?;
        let mut state = self.state.lock().unwrap();
        state.pre_dumped_uid = None;
        if self.dumpstate().is_some() {
            eprintln!(
                "app host: bugreport: 'dumpstate' is already running. Cannot pre-dump data while another operation is currently in progress."
            );
            return Ok(());
        }
        let Some(dumpstate) = self.start_dumpstate() else {
            eprintln!("app host: bugreport: Unable to get bugreport service");
            return Ok(());
        };
        let mut data = Parcel::new();
        ds::PreDumpUiData {
            calling_package: pkg.map(str::to_string),
        }
        .write(&mut data);
        let called = Self::call(&dumpstate, ds::PRE_DUMP_UI_DATA, &data);
        // Stopped whatever the call did, so the next caller can start it.
        self.stop_dumpstate();
        match called {
            Some(Ok(())) => state.pre_dumped_uid = Some(uid),
            Some(Err(e)) => return Err(e),
            None => {}
        }
        Ok(())
    }

    /// `startBugreport`.
    fn start(&self, a: ds::StartBugreport<ReceivedFd>, uid: i32) -> Result<()> {
        let pkg = a.calling_package.ok_or_else(null)?;
        let Some(Binder::Handle(listener)) = a.listener else {
            return Err(null());
        };
        if !MODES.contains(&a.bugreport_mode) {
            let message = format!("Unknown bugreport mode: {}", a.bugreport_mode);
            eprintln!("app host: bugreport: {message}");
            return Err(Exception::illegal_argument(message));
        }
        let unknown = a.bugreport_flags
            & !(FLAG_USE_PREDUMPED_UI_DATA | FLAG_DEFER_CONSENT | FLAG_KEEP_BUGREPORT_ON_RETRIEVAL);
        if unknown != 0 {
            let message = format!("Unknown bugreport flags: {unknown}");
            eprintln!("app host: bugreport: {message}");
            return Err(Exception::illegal_argument(message));
        }
        self.enforce(
            Some(&pkg),
            uid,
            a.bugreport_mode == BUGREPORT_MODE_TELEPHONY,
        )?;
        self.ensure_user_can_take(uid, a.bugreport_mode)?;
        eprintln!("app host: bugreport: Starting bugreport for {pkg} / {uid}");
        let request = Start {
            uid,
            pkg,
            bugreport_fd: self.file(a.bugreport_fd)?,
            screenshot_fd: self.file(a.screenshot_fd)?,
            mode: a.bugreport_mode,
            flags: a.bugreport_flags,
            listener: Arc::new(self.host.process.strong(listener)),
            screenshot: a.is_screenshot_requested,
        };
        if !self.flag(ASYNC_START_BUGREPORT)? {
            let mut state = self.state.lock().unwrap();
            return self.start_locked(&mut state, request, uid);
        }
        // The start runs on its own thread; the call returns once that
        // holds the lock.
        let (handoff, handed) = mpsc::channel();
        let me = self.me.clone();
        std::thread::spawn(move || {
            let Some(service) = me.upgrade() else {
                return;
            };
            let mut state = service.state.lock().unwrap();
            // The call may have given up waiting.
            let _ = handoff.send(());
            let listener = request.listener.clone();
            // The thread calls as the system.
            if let Err(e) = service.start_locked(&mut state, request, SYSTEM_UID) {
                eprintln!(
                    "app host: bugreport: Cannot start a new bugreport due to an unknown error: {}",
                    e.message
                );
                Self::report_error(&listener, ERROR_RUNTIME_ERROR);
            }
        });
        if handed
            .recv_timeout(DEFAULT_BUGREPORT_SERVICE_TIMEOUT)
            .is_err()
        {
            eprintln!(
                "app host: bugreport: Unexpectedly interrupted waiting for startBugreportLocked"
            );
        }
        Ok(())
    }

    /// `startBugreportLocked`; `calling` is the uid the binder calls are
    /// made as.
    fn start_locked(
        &self,
        state: &mut MutexGuard<'_, State>,
        r: Start,
        calling: i32,
    ) -> Result<()> {
        if self.dumpstate().is_some() {
            eprintln!(
                "app host: bugreport: 'dumpstate' is already running. Cannot start a new bugreport while another operation is currently in progress."
            );
            Self::report_error(&r.listener, ERROR_ANOTHER_REPORT_IN_PROGRESS);
            return Ok(());
        }
        let mut flags = r.flags;
        if flags & FLAG_USE_PREDUMPED_UI_DATA != 0 {
            match state.pre_dumped_uid {
                None => {
                    flags &= !FLAG_USE_PREDUMPED_UI_DATA;
                    eprintln!(
                        "app host: bugreport: Ignoring BUGREPORT_FLAG_USE_PREDUMPED_UI_DATA. No pre-dumped data is available."
                    );
                }
                Some(pre) if pre != r.uid => {
                    flags &= !FLAG_USE_PREDUMPED_UI_DATA;
                    eprintln!(
                        "app host: bugreport: Ignoring BUGREPORT_FLAG_USE_PREDUMPED_UI_DATA. Data was pre-dumped by a different UID."
                    );
                }
                Some(_) => {}
            }
        }
        let deferred = flags & FLAG_DEFER_CONSENT != 0;
        let keep = flags & FLAG_KEEP_BUGREPORT_ON_RETRIEVAL != 0;
        let Some(dumpstate) = self.start_dumpstate() else {
            eprintln!("app host: bugreport: Unable to get bugreport service");
            Self::report_error(&r.listener, ERROR_RUNTIME_ERROR);
            return Ok(());
        };
        let consentless = self.consentless()?;
        let skip = self
            .files
            .lock()
            .unwrap()
            .can_skip_consent(consentless, &r.pkg, !deferred);
        let mine = self.listen(
            state,
            Listening {
                client: r.listener.clone(),
                caller: (r.uid, Some(r.pkg.clone())),
                report_finished_file: deferred,
                keep,
                consent_granted: !deferred && !skip,
                deferred,
            },
            &dumpstate,
        );
        let mut data = Parcel::new();
        ds::StartBugreport {
            calling_uid: r.uid,
            calling_package: Some(r.pkg.clone()),
            bugreport_fd: r.bugreport_fd,
            screenshot_fd: r.screenshot_fd,
            bugreport_mode: r.mode,
            bugreport_flags: flags,
            listener: Some(mine),
            is_screenshot_requested: r.screenshot,
            skip_user_consent: skip,
        }
        .write(&mut data);
        match Self::call(&dumpstate, ds::START_BUGREPORT, &data) {
            Some(result) => result,
            // Stopped, or the next caller finds it running; the listener
            // hears of it from the death (`cancelBugreport`).
            None => {
                self.enforce(Some(&r.pkg), calling, true)?;
                self.cancel_locked(calling, &r.pkg)
            }
        }
    }

    /// `cancelBugreport`.
    fn cancel(&self, pkg: Option<&str>, uid: i32) -> Result<()> {
        self.enforce(pkg, uid, true)?;
        let pkg = pkg.unwrap_or_default();
        eprintln!("app host: bugreport: Cancelling bugreport for {pkg} / {uid}");
        let _state = self.state.lock().unwrap();
        self.cancel_locked(uid, pkg)
    }

    /// dumpstate's cancel, then `bugreportd` stopped. One dumpstate refuses
    /// is the caller's, and it is not stopped.
    fn cancel_locked(&self, uid: i32, pkg: &str) -> Result<()> {
        let Some(dumpstate) = self.dumpstate() else {
            eprintln!(
                "app host: bugreport: cancelBugreport: Could not find native dumpstate service"
            );
            return Ok(());
        };
        let mut data = Parcel::new();
        ds::CancelBugreport {
            calling_uid: uid,
            calling_package: Some(pkg.to_string()),
        }
        .write(&mut data);
        match Self::call(&dumpstate, ds::CANCEL_BUGREPORT, &data) {
            Some(result) => result?,
            None => eprintln!("app host: bugreport: RemoteException in cancelBugreport"),
        }
        self.stop_dumpstate();
        Ok(())
    }

    /// `retrieveBugreport`.
    fn retrieve(&self, a: ds::RetrieveBugreport<ReceivedFd>, uid: i32) -> Result<()> {
        let pkg = a.calling_package;
        self.enforce(pkg.as_deref(), uid, false)?;
        let pkg = pkg.unwrap_or_default();
        let Some(Binder::Handle(listener)) = a.listener else {
            return Err(null());
        };
        let listener = Arc::new(self.host.process.strong(listener));
        let bugreport_fd = self.file(a.bugreport_fd)?;
        let file = a.bugreport_file.unwrap_or_default();
        eprintln!("app host: bugreport: Retrieving bugreport for {pkg} / {uid}");
        if let Err(e) = self.ensure_generated(uid, &pkg, a.user_id, &file)? {
            eprintln!("app host: bugreport: {e}");
            Self::report_error(&listener, ERROR_NO_BUGREPORT_TO_RETRIEVE);
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if self.dumpstate().is_some() {
            eprintln!(
                "app host: bugreport: 'dumpstate' is already running. Cannot retrieve a bugreport while another one is currently in progress."
            );
            Self::report_error(&listener, ERROR_ANOTHER_REPORT_IN_PROGRESS);
            return Ok(());
        }
        let Some(dumpstate) = self.start_dumpstate() else {
            eprintln!("app host: bugreport: Unable to get bugreport service");
            Self::report_error(&listener, ERROR_RUNTIME_ERROR);
            return Ok(());
        };
        let consentless = self.consentless()?;
        let v2 = self.flag(ONBOARDING_BUGREPORT_V2)?;
        let (skip, keep) = {
            let mut files = self.files.lock().unwrap();
            (
                files.can_skip_consent(consentless, &pkg, false),
                v2 && files.to_persist.contains(&file),
            )
        };
        let mine = self.listen(
            &mut state,
            Listening {
                client: listener,
                caller: (uid, Some(pkg.clone())),
                report_finished_file: true,
                keep: false,
                consent_granted: !skip,
                deferred: true,
            },
            &dumpstate,
        );
        let mut data = Parcel::new();
        ds::RetrieveBugreport {
            calling_uid: uid,
            calling_package: Some(pkg),
            user_id: a.user_id,
            bugreport_fd,
            bugreport_file: Some(file),
            keep_bugreport_on_retrieval: keep,
            skip_user_consent: skip,
            listener: Some(mine),
        }
        .write(&mut data);
        match Self::call(&dumpstate, ds::RETRIEVE_BUGREPORT, &data) {
            Some(result) => result,
            None => {
                eprintln!("app host: bugreport: RemoteException in retrieveBugreport");
                Ok(())
            }
        }
    }

    /// `ensureCallerPreviouslyGeneratedFile`: the caller's file, taken from
    /// the mapping unless it is kept; else the reason it is none.
    fn ensure_generated(
        &self,
        uid: i32,
        pkg: &str,
        user: i32,
        file: &str,
    ) -> Result<std::result::Result<(), String>> {
        let invalid =
            || format!("File {file} was not generated on behalf of calling package {pkg}");
        let v2 = self.flag(ONBOARDING_BUGREPORT_V2)?;
        let mapping = self.mapping_file()?;
        let mut files = self.files.lock().unwrap();
        if !v2 {
            let caller: Caller = (uid, Some(pkg.to_string()));
            let Some(at) = files.files.iter().position(|(c, _)| *c == caller) else {
                return Ok(Err(invalid()));
            };
            let list = &mut files.files[at].1;
            let Some(i) = list.iter().position(|f| f == file) else {
                return Ok(Err(invalid()));
            };
            list.remove(i);
            if list.is_empty() {
                files.files.remove(at);
            }
            return Ok(Ok(()));
        }
        let Some(uid_for_user) = self.package_uid(pkg, user)? else {
            return Ok(Err(invalid()));
        };
        if uid_for_user != uid
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "{pkg} does not hold the INTERACT_ACROSS_USERS permission to access cross-user bugreports."
            )));
        }
        if !files.read {
            files.read_mapping(&mapping);
        }
        let keep = files.to_persist.iter().any(|f| f == file);
        let caller: Caller = (uid_for_user, Some(pkg.to_string()));
        let Some((_, list)) = files.files.iter_mut().find(|(c, _)| *c == caller) else {
            return Ok(Err(invalid()));
        };
        let Some(i) = list.iter().position(|f| f == file) else {
            return Ok(Err(invalid()));
        };
        if !keep {
            list.remove(i);
        }
        Ok(Ok(()))
    }

    /// `getPackageUidAsUser`: none for a package not installed.
    fn package_uid(&self, pkg: &str, user: i32) -> Result<Option<i32>> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let uid = query
            .package_uid(pkg, 0, user)
            .map_err(|e| self.host.unimplemented_named("bugreport", e.0))??;
        Ok((uid >= 0).then_some(uid))
    }

    /// A `DumpstateListener` for dumpstate to call, made the current one,
    /// and linked to dumpstate's death.
    fn listen(
        &self,
        state: &mut MutexGuard<'_, State>,
        listening: Listening,
        dumpstate: &Arc<Strong>,
    ) -> Binder {
        let id = self.next_listener.fetch_add(1, Ordering::Relaxed);
        if let Some(current) = state.current {
            eprintln!(
                "app host: bugreport: setCurrentDumpstateListenerLocked({id}): called when mCurrentDumpstateListener is already set ({current})"
            );
        }
        state.current = Some(id);
        let listener = Arc::new(Listener {
            service: self.me.clone(),
            id,
            listening,
            _dumpstate: dumpstate.clone(),
            done: AtomicBool::new(false),
        });
        let died = listener.clone();
        self.host.process.link_to_death(
            dumpstate,
            Box::new(move || {
                // Its last callbacks are given a moment to arrive.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_secs(1));
                    let Some(service) = died.service.upgrade() else {
                        return;
                    };
                    let _state = service.state.lock().unwrap();
                    if !died.done.load(Ordering::Acquire) {
                        eprintln!(
                            "app host: bugreport: IDumpstate likely crashed. Notifying listener"
                        );
                        Self::report_error(&died.listening.client, ERROR_RUNTIME_ERROR);
                    }
                });
            }),
        );
        self.host.process.add_service(listener)
    }
}

/// What `startBugreport` passes on to `startBugreportLocked`.
struct Start {
    uid: i32,
    pkg: String,
    bugreport_fd: SentFd,
    screenshot_fd: SentFd,
    mode: i32,
    flags: i32,
    listener: Arc<Strong>,
    screenshot: bool,
}

/// What a `DumpstateListener` reports to.
struct Listening {
    client: Arc<Strong>,
    caller: Caller,
    report_finished_file: bool,
    keep: bool,
    consent_granted: bool,
    deferred: bool,
}

/// `DumpstateListener`: dumpstate's calls passed on to the caller's
/// listener, a report's end recorded.
struct Listener {
    service: Weak<Bugreport>,
    id: u64,
    listening: Listening,
    /// Held while dumpstate's death is linked.
    _dumpstate: Arc<Strong>,
    done: AtomicBool,
}

impl Listener {
    /// `releaseItselfLocked`.
    fn release(&self, service: &Bugreport) {
        let mut state = service.state.lock().unwrap();
        self.done.store(true, Ordering::Release);
        if state.current == Some(self.id) {
            state.current = None;
        } else {
            eprintln!(
                "app host: bugreport: releaseItselfLocked(): {} is finished, but current listener is {:?}",
                self.id, state.current
            );
        }
    }

    /// `onFinished`'s record of the file and the consent.
    fn finished(&self, service: &Bugreport, file: &str) -> Result<()> {
        let v2 = service.flag(ONBOARDING_BUGREPORT_V2)?;
        let consentless = service.consentless()?;
        let mapping = service.mapping_file()?;
        let mut files = service.files.lock().unwrap();
        let (caller, l) = (&self.listening.caller, &self.listening);
        if l.report_finished_file {
            files.add_mapping(caller.clone(), file.to_string());
            if v2 {
                if l.keep && !files.to_persist.iter().any(|f| f == file) {
                    files.to_persist.push(file.to_string());
                }
                files.write_mapping(&mapping);
            }
        }
        let pkg = caller.1.as_deref().unwrap_or_default();
        files.log_consent(consentless, pkg, l.consent_granted, l.deferred);
        Ok(())
    }
}

impl Service for Listener {
    fn descriptor(&self) -> &str {
        dl::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(service) = self.service.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        let mut data = Parcel::new();
        match call.code {
            dl::ON_PROGRESS => {
                dl::OnProgress::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?
                    .write(&mut data);
            }
            dl::ON_ERROR => {
                let a = dl::OnError::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                eprintln!("app host: bugreport: onError(): {}", a.error_code);
                self.release(&service);
                a.write(&mut data);
            }
            dl::ON_FINISHED => {
                let a = dl::OnFinished::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let file = a.bugreport_file.clone().unwrap_or_default();
                eprintln!("app host: bugreport: onFinished(): {file}");
                self.release(&service);
                if let Err(e) = self.finished(&service, &file) {
                    eprintln!("app host: bugreport: {}", e.message);
                }
                a.write(&mut data);
            }
            dl::ON_SCREENSHOT_TAKEN => {
                dl::OnScreenshotTaken::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?
                    .write(&mut data);
            }
            dl::ON_UI_INTENSIVE_BUGREPORT_DUMPS_FINISHED => {
                dl::OnUiIntensiveBugreportDumpsFinished::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?
                    .write(&mut data);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        if let Err(status) = self.listening.client.transact(call.code, &data, true) {
            eprintln!("app host: bugreport: the caller's listener: {status}");
        }
        Ok(Parcel::new())
    }
}

impl Service for Bugreport {
    fn descriptor(&self) -> &str {
        ds::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            ds::PRE_DUMP_UI_DATA => {
                let a = ds::PreDumpUiData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.pre_dump(a.calling_package.as_deref(), uid)
                    .map(|()| ds::write_pre_dump_ui_data_reply(&mut p))
            }
            ds::START_BUGREPORT => {
                let a = ds::StartBugreport::<ReceivedFd>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.start(a, uid)
                    .map(|()| ds::write_start_bugreport_reply(&mut p))
            }
            ds::CANCEL_BUGREPORT => {
                let a = ds::CancelBugreport::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.cancel(a.calling_package.as_deref(), uid)
                    .map(|()| ds::write_cancel_bugreport_reply(&mut p))
            }
            ds::RETRIEVE_BUGREPORT => {
                let a = ds::RetrieveBugreport::<ReceivedFd>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.retrieve(a, uid)
                    .map(|()| ds::write_retrieve_bugreport_reply(&mut p))
            }
            crate::shell::SHELL_COMMAND_TRANSACTION | DUMP_TRANSACTION => Err(self
                .host
                .unimplemented_named("bugreport", "its dump and shell command (#1943)")),
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root of this test's own: the clock's resolution lets two tests
    /// name the same one, and a killed earlier run's is another run's
    /// (#1917).
    fn tmp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        loop {
            let dir = std::env::temp_dir().join(format!(
                "aim-bugreport-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => panic!("{}: {e}", dir.display()),
            }
        }
    }

    /// `writeBugreportDataLocked` and `readBugreportMappingLocked`: the
    /// files of each caller and those kept, read back as written.
    #[test]
    fn the_mapping_is_read_as_written() {
        let path = tmp().join("bugreport-mapping.xml");
        let mut files = Files::default();
        files.add_mapping((10_100, Some("com.a".into())), "a.zip".into());
        files.add_mapping((10_100, Some("com.a".into())), "b.zip".into());
        files.add_mapping((10_101, Some("com.b".into())), "c.zip".into());
        files.to_persist.push("b.zip".into());
        files.write_mapping(&path);

        let mut read = Files::default();
        read.read_mapping(&path);
        assert!(read.read);
        assert_eq!(read.to_persist, vec!["b.zip".to_string()]);
        let mut got = read.files.clone();
        got.sort_by_key(|(c, _)| c.0);
        for (_, f) in &mut got {
            f.sort();
        }
        assert_eq!(
            got,
            vec![
                (
                    (10_100, Some("com.a".to_string())),
                    vec!["a.zip".to_string(), "b.zip".to_string()]
                ),
                (
                    (10_101, Some("com.b".to_string())),
                    vec!["c.zip".to_string()]
                ),
            ]
        );
    }

    /// A mapping file that does not read is deleted.
    #[test]
    fn a_bad_mapping_is_deleted() {
        let path = tmp().join("bugreport-mapping.xml");
        std::fs::write(&path, b"<other />").unwrap();
        let mut files = Files::default();
        files.read_mapping(&path);
        assert!(!files.read);
        assert!(!path.exists());
        let mut absent = Files::default();
        absent.read_mapping(&path);
        assert!(!absent.read);
    }

    /// `canSkipConsentScreen` and `logConsentGrantedForCaller`: a consent
    /// covers deferred reports for two minutes, a full one once.
    #[test]
    fn a_consent_covers_reports_for_its_grace_period() {
        let mut files = Files::default();
        assert!(!files.can_skip_consent(true, "com.a", false));
        // A deferred report's consent covers one full report too.
        files.log_consent(true, "com.a", true, true);
        assert!(files.can_skip_consent(true, "com.a", true));
        files.log_consent(true, "com.a", false, false);
        assert!(!files.can_skip_consent(true, "com.a", true));
        assert!(files.can_skip_consent(true, "com.a", false));
        assert!(!files.can_skip_consent(false, "com.a", false));
        files.consents[0].1.0 -= CONSENTLESS_GRACE_PERIOD_MS + 1;
        assert!(!files.can_skip_consent(true, "com.a", false));
        assert!(files.consents.is_empty());
    }
}

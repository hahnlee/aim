//! Native install preparation and ordered execution contracts (#986).
//! A completed boot scan is insufficient to authorize installation: live
//! permission, app-data, process, code-location and publication owners participate.
use super::{Record, Session};
use crate::package::{pkg::AndroidPackage, sign::SigningDetails, write::Apks};
use aim_binder_host::parcel::Exception;
use std::collections::BTreeSet;

#[derive(Debug)]
pub struct Failure {
    pub legacy_status: i32,
    pub committed: bool,
    pub message: String,
}
impl Failure {
    fn after_commit(message: impl Into<String>) -> Self {
        Self {
            legacy_status: -110,
            committed: true,
            message: message.into(),
        }
    }
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            legacy_status: -2,
            committed: false,
            message: message.into(),
        }
    }
}
fn owner_failure(stage: &str, mut error: Exception) -> Exception {
    error.message = format!("{stage} (exception {}): {}", error.code, error.message);
    error
}
fn stage_failure(stage: &str, mut failure: Failure) -> Failure {
    failure.message = format!("{stage}: {}", failure.message);
    failure
}
/// Code validated by the real parser and APK signature verifier. This has not
/// passed live installation policy, reconciliation or filesystem preparation.
#[derive(Clone)]
pub struct VerifiedCode {
    pub session: Session,
    pub record: Record,
    pub package: AndroidPackage,
    pub signing: SigningDetails,
}
/// Validate every member before giving any member to an installation owner.
/// `paths` identifies the actual normalized staging directories, not installed
/// package paths or the shadow model's previously observed state.
pub fn verify_batch(
    apks: &Apks,
    members: Vec<(Session, Record, String)>,
    lite: &super::native::LitePolicy,
) -> Result<Vec<VerifiedCode>, Failure> {
    let mut names = BTreeSet::new();
    let mut result = Vec::with_capacity(members.len());
    for (session, record, path) in members {
        if !session.prepared || !session.sealed || session.destroyed {
            return Err(Failure::invalid(format!(
                "Session {} is not an immutable prepared install",
                session.id
            )));
        }
        if session.parameters.multi_package {
            return Err(Failure::invalid(
                "A multi-package parent is not an APK candidate",
            ));
        }
        if !matches!(record.params.mode, 1 | 2) {
            return Err(Failure::invalid("Invalid native install mode"));
        }
        if session.parameters.install_flags & 0x20000 != 0 {
            return Err(Failure {
                legacy_status: -110,
                committed: false,
                message: "Native APEX installation validation owner unavailable".into(),
            });
        }
        let host = (apks.files)(&path)
            .ok_or_else(|| Failure::invalid("Install staging VFS path unavailable"))?;
        for entry in
            std::fs::read_dir(&host).map_err(|error| Failure::invalid(error.to_string()))?
        {
            let entry = entry.map_err(|error| Failure::invalid(error.to_string()))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Failure::invalid("Invalid staged filename encoding"))?;
            if entry.path().is_dir()
                || name.ends_with(".removed")
                || name.ends_with(".idsig")
                || name.ends_with("app.metadata")
                || name.ends_with(".digests")
                || name.ends_with(".digests.signature")
                || lite
                    .art_managed_extensions
                    .iter()
                    .any(|extension| name.ends_with(extension))
            {
                continue;
            }
            crate::package::parse::lite::package_name(&entry.path(), &lite.environment)
                .map_err(|error| Failure::invalid(error.to_string()))?;
            if !name.ends_with(".apk") {
                return Err(Failure {
                    legacy_status: -110,
                    committed: false,
                    message: "Native staged APK filename normalization owner unavailable".into(),
                });
            }
        }
        let package = apks.checked_parsed_path(&path, 0).map_err(|error| match error {
            crate::package::parse::Error::OlderSdk(message) => Failure { legacy_status: -12, committed: false, message },
            error => Failure::invalid(error.to_string()),
        })?;
        if record
            .params
            .app_package_name
            .as_deref()
            .is_some_and(|name| name != package.package_name)
        {
            return Err(Failure::invalid(format!(
                "Package name {} does not match session {}",
                package.package_name, session.id
            )));
        }
        if !names.insert(package.package_name.clone()) {
            return Err(Failure::invalid(format!(
                "Multi-package install contains {} more than once",
                package.package_name
            )));
        }
        let signing = apks.signing_details(&package).map_err(|message| Failure {
            legacy_status: -103,
            committed: false,
            message,
        })?;
        result.push(VerifiedCode {
            session,
            record,
            package,
            signing,
        });
    }
    Ok(result)
}

/// A prepared installation owns reserved code locations, admitted settings,
/// native libraries, app data and rollback resources. Dropping it before commit
/// must release its own reservations; the native owner implements that lifetime.
/// It can only report success after durable state and query publication complete.
pub trait PreparedInstall: Send {
    fn commit(self: Box<Self>) -> Result<PublishedInstall, Failure>;
}
/// Constructible only by an owner with evidence for the published native state.
/// Callers independently inspect that generation before sending STATUS_SUCCESS.
pub struct PublishedInstall {
    pub generation: u64,
    pub packages: Vec<InstalledPackage>,
    /// Exact verified sessions, never inferred from installed package names.
    pub verified_sessions: Vec<VerifiedCode>,
    pub new_installations: std::collections::BTreeSet<(String,u32)>,
}
pub struct InstalledPackage {
    pub name: String,
    pub version_code: i64,
    pub user: u32,
}
/// Live installation owners are mandatory. No default implementation may use
/// original PMS or return a successful receipt without preparing/installing.
pub trait Owners: Send + Sync {
    /// Applies live installer policy, permission verification, UID/signature/
    /// library reconciliation and reserves the whole batch atomically.
    fn prepare(&self, code: Vec<VerifiedCode>) -> Result<Box<dyn PreparedInstall>, Failure>;
    /// Reads the native published generation and effective package installation.
    /// Must reject a stale or incomplete publication receipt.
    fn confirm_publication(&self, receipt: &PublishedInstall) -> Result<(), Failure>;
    /// Performs actual post-install process/dexopt/permission/broadcast effects
    /// and delivers the original IntentSender status. Errors stay observable.
    fn finish(&self, receipt: PublishedInstall) -> Result<(), Exception>;
}

pub enum Error {
    Pending,
    Install(Failure),
    Owner { committed: bool, error: Exception },
}
/// An install cannot reach its success callback through only parsed code or
/// a persisted boot scan: every required live owner completes in this order.
pub fn install(
    owners: &dyn Owners,
    code: Vec<VerifiedCode>,
    source: &super::native::QuerySource,
) -> Result<(), Error> {
    let before = source()
        .map_err(|error| Error::Owner {
            committed: false,
            error: owner_failure("Native install initial query", error),
        })?
        .generation;
    let expected: Vec<_> = code
        .iter()
        .map(|code| {
            (
                if code.package.static_shared_library_name.is_some() {
                    format!(
                        "{}_{}",
                        code.package.package_name, code.package.static_shared_lib_version
                    )
                } else {
                    code.package.package_name.clone()
                },
                ((code.package.version_code_major as i64) << 32)
                    | (code.package.version_code as u32 as i64),
                code.session.user,
                code.signing.signatures.clone(),
            )
        })
        .collect();
    let prepared = owners.prepare(code).map_err(|error| Error::Install(stage_failure("Native install preparation", error)))?;
    let receipt = prepared.commit().map_err(|error| Error::Install(stage_failure("Native install commit", error)))?;
    let published = source().map_err(|error| Error::Owner {
        committed: true,
        error: owner_failure("Native install published query", error),
    })?;
    if receipt.generation <= before
        || published.generation < receipt.generation
        || receipt.packages.is_empty()
    {
        return Err(Error::Install(Failure::after_commit(
            "Installation publication receipt is stale or incomplete",
        )));
    }
    for package in &receipt.packages {
        if !expected
            .iter()
            .any(|(name, version, _, _)| name == &package.name && *version == package.version_code)
        {
            return Err(Error::Install(Failure::after_commit(
                "Installation receipt contains a foreign package",
            )));
        }
        let setting = published.packages.get(&package.name).ok_or_else(|| {
            Error::Install(Failure::after_commit("Receipt package not published"))
        })?;
        if !crate::package::info::user_state(setting, package.user as i32).installed {
            return Err(Error::Install(Failure::after_commit(
                "Receipt user is not installed",
            )));
        }
    }
    for (name, version, user, signatures) in expected {
        if !receipt.packages.iter().any(|package| {
            package.name == name && package.version_code == version && package.user == user
        }) {
            return Err(Error::Install(Failure::after_commit(
                "Installation receipt omitted a validated member",
            )));
        }
        let setting = published.packages.get(&name).ok_or_else(|| {
            Error::Install(Failure::after_commit(
                "Validated install is missing from native publication",
            ))
        })?;
        if setting.version_code != version
            || !crate::package::info::user_state(setting, user as i32).installed
            || setting.pkg.is_none()
            || setting
                .signatures
                .as_ref()
                .is_none_or(|known| known.signatures != signatures)
        {
            return Err(Error::Install(Failure::after_commit(
                "Native publication does not contain the validated installed code",
            )));
        }
    }
    owners
        .confirm_publication(&receipt)
        .map_err(|mut failure| {
            failure.committed = true;
            Error::Install(failure)
        })?;
    owners.finish(receipt).map_err(|error| Error::Owner {
        committed: true,
        error: owner_failure("Native install completion", error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Prepared {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail: bool,
    }
    impl PreparedInstall for Prepared {
        fn commit(self: Box<Self>) -> Result<PublishedInstall, Failure> {
            self.events.lock().unwrap().push("commit");
            if self.fail {
                return Err(Failure::invalid("disk commit failed"));
            }
            Ok(PublishedInstall {
                generation: 4,
                packages: vec![],
                verified_sessions: Vec::new(),
                new_installations: Default::default(),
            })
        }
    }
    struct Owner {
        events: Arc<Mutex<Vec<&'static str>>>,
        fail_commit: bool,
        fail_confirm: bool,
    }
    impl Owners for Owner {
        fn prepare(&self, _: Vec<VerifiedCode>) -> Result<Box<dyn PreparedInstall>, Failure> {
            self.events.lock().unwrap().push("prepare");
            Ok(Box::new(Prepared {
                events: self.events.clone(),
                fail: self.fail_commit,
            }))
        }
        fn confirm_publication(&self, _: &PublishedInstall) -> Result<(), Failure> {
            self.events.lock().unwrap().push("confirm");
            if self.fail_confirm {
                Err(Failure::invalid("stale generation"))
            } else {
                Ok(())
            }
        }
        fn finish(&self, _: PublishedInstall) -> Result<(), Exception> {
            self.events.lock().unwrap().push("finish");
            Ok(())
        }
    }
    #[test]
    fn rejected_durable_commit_and_stale_publication_never_deliver_success() {
        for (commit, confirm, expected) in [
            (true, false, vec!["prepare", "commit"]),
            (false, true, vec!["prepare", "commit"]),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let owner = Owner {
                events: events.clone(),
                fail_commit: commit,
                fail_confirm: confirm,
            };
            let source: super::super::native::QuerySource =
                Arc::new(|| Ok(Arc::new(crate::package::model::State::default())));
            match install(&owner, vec![], &source) {
                Err(Error::Install(failure)) => assert_eq!(failure.committed, !commit),
                _ => panic!("Failed install unexpectedly completed"),
            }
            assert_eq!(*events.lock().unwrap(), expected);
        }
    }
}

/// Filesystem, image and service owners retained for one live installation.
/// Implementations reserve actual code paths and retain cleanup ownership until
/// the committed generation is handed to the runtime completion owner.
pub trait Reservation: Send + Sync {
    fn requests(&self) -> Result<Vec<crate::package::scan::live_install::Request>, Failure>;
    fn users(&self) -> &[crate::package::scan::User];
    fn build_debuggable(&self) -> bool;
    fn complete_metadata(
        &self,
        admission: crate::package::scan::live_install::Admission,
        apks: &Apks,
    ) -> Result<crate::package::scan::live_install::CompletedAdmission, Failure>;
    fn prepare_runtime(
        &self,
        admission: &mut crate::package::scan::live_install::CompletedAdmission,
    ) -> Result<(), Failure>;
    fn cross_user_suspensions(&self) -> bool;
    fn mark_committed(&self, snapshot: &std::sync::Arc<crate::package::scan_snapshot::Snapshot>);
    fn publication_finished(
        &self,
        snapshot: &std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    ) -> Result<(), Exception>;
}
pub trait Environment: Send + Sync {
    fn reserve(
        &self,
        code: Vec<VerifiedCode>,
        base: &std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    ) -> Result<Box<dyn Reservation>, Failure>;
    fn publish_queries(
        &self,
        snapshot: &std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    ) -> Result<(), String>;
    fn finish(&self, receipt: PublishedInstall) -> Result<(), Exception>;
}
/// Concrete native admission, persistence and publication coordinator. External
/// callbacks own real files/app data/permissions; core identity and settings
/// mutation is always performed here by the native SigningScan and Store.
pub struct Native {
    pub snapshots: std::sync::Arc<crate::package::scan_snapshot::Store>,
    pub disk: std::sync::Arc<std::sync::Mutex<crate::package::owner::Store>>,
    pub apks: std::sync::Arc<Apks>,
    pub environment: std::sync::Arc<dyn Environment>,
    pub publication_gate: std::sync::Arc<std::sync::Mutex<()>>,
}
pub(crate) struct NativePrepared {
    publication_gate: std::sync::Arc<std::sync::Mutex<()>>,
    snapshots: std::sync::Arc<crate::package::scan_snapshot::Store>,
    disk: std::sync::Arc<std::sync::Mutex<crate::package::owner::Store>>,
    environment: std::sync::Arc<dyn Environment>,
    base: std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    reservation: Box<dyn Reservation>,
    admission: crate::package::scan::live_install::CompletedAdmission,
    verified_sessions: Vec<VerifiedCode>,
}
pub(crate) struct PublicationPermit<'a> {
    gate: &'a std::sync::Arc<std::sync::Mutex<()>>,
    _guard: std::sync::MutexGuard<'a, ()>,
}
impl<'a> PublicationPermit<'a> {
    fn acquire(gate: &'a std::sync::Arc<std::sync::Mutex<()>>) -> Self {
        Self { gate, _guard: gate.lock().unwrap() }
    }
    pub(crate) fn matches_gate(&self, gate:&std::sync::Arc<std::sync::Mutex<()>>) -> bool {
        std::sync::Arc::ptr_eq(self.gate,gate)
    }
}
impl Native {
    pub(crate) fn publication_permit(&self) -> PublicationPermit<'_> {
        PublicationPermit::acquire(&self.publication_gate)
    }
    pub(crate) fn prepare_native(&self, code: Vec<VerifiedCode>) -> Result<NativePrepared, Failure> {
        let base = self.snapshots.capture();
        let verified_sessions = code.clone();
        let reservation = self.environment.reserve(code, &base).map_err(|error| stage_failure("Reservation", error))?;
        let requests = reservation.requests().map_err(|error| stage_failure("Admission requests", error))?;
        let admission = base
            .owner()
            .prepare_live_installs(
                requests,
                reservation.users(),
                reservation.build_debuggable(),
                &self.apks.files,
            )
            .map_err(|error| Failure {
                legacy_status: error.status,
                committed: false,
                message: error.message,
            })?;
        let mut admission = reservation.complete_metadata(admission, &self.apks).map_err(|error| stage_failure("Scan metadata", error))?;
        reservation.prepare_runtime(&mut admission).map_err(|error| stage_failure("Runtime and permission preparation", error))?;
        Ok(NativePrepared {
            publication_gate: self.publication_gate.clone(),
            snapshots: self.snapshots.clone(),
            disk: self.disk.clone(),
            environment: self.environment.clone(),
            base,
            reservation,
            admission,
            verified_sessions,
        })
    }
}
impl Owners for Native {
    fn prepare(&self, code: Vec<VerifiedCode>) -> Result<Box<dyn PreparedInstall>, Failure> {
        self.prepare_native(code).map(|prepared|Box::new(prepared) as Box<dyn PreparedInstall>)
    }
    fn confirm_publication(&self, receipt: &PublishedInstall) -> Result<(), Failure> {
        if self.snapshots.capture().version() < receipt.generation {
            return Err(Failure::after_commit(
                "Native scan generation is not published",
            ));
        }
        Ok(())
    }
    fn finish(&self, receipt: PublishedInstall) -> Result<(), Exception> {
        self.environment.finish(receipt)
    }
}
pub(crate) struct NativeCommitted {
    pub(crate) receipt: PublishedInstall,
    snapshot: std::sync::Arc<crate::package::scan_snapshot::Snapshot>,
    reservation: Box<dyn Reservation>,
}
impl NativeCommitted {
    pub(crate) fn finish(self) -> Result<PublishedInstall, Failure> {
        self.reservation.publication_finished(&self.snapshot).map_err(|error|
            Failure::after_commit(owner_failure("Committed publication effects", error).message))?;
        Ok(self.receipt)
    }
}
impl PreparedInstall for NativePrepared {
    fn commit(self: Box<Self>) -> Result<PublishedInstall, Failure> {
        let gate=self.publication_gate.clone();
        let permit=PublicationPermit::acquire(&gate);
        let committed=(*self).commit_under_permit(&permit)?;
        drop(permit);
        committed.finish()
    }
}
impl NativePrepared {
    pub(crate) fn commit_under_permit(self, permit:&PublicationPermit<'_>) -> Result<NativeCommitted, Failure> {
        if !permit.matches_gate(&self.publication_gate) {
            return Err(Failure::invalid("install publication permit belongs to another owner"));
        }
        let NativePrepared {
            publication_gate: _,
            snapshots,
            disk,
            environment,
            base,
            reservation,
            admission,
            verified_sessions,
        } = self;
        let users: Vec<u32> = reservation
            .users()
            .iter()
            .map(|user| {
                u32::try_from(user.id).map_err(|_| Failure::invalid("Negative installed user"))
            })
            .collect::<Result<_, _>>()?;
        // Metadata writers use this same gate. Durable/canonical installation
        // must not become visible to them before its query projection exists.
        let mut disk = disk.lock().unwrap();
        let result = admission.persist_publish_install(
            &snapshots,
            &base,
            &mut disk,
            &users,
            reservation.cross_user_suspensions(),
        );
        drop(disk);
        let publication = match result {
            Ok(publication) => publication,
            Err(crate::package::scan::live_install::PublicationFailure::Persist {
                error, ..
            }) => {
                return Err(match error {
                    crate::package::scan_snapshot::CommitError::Snapshot(error) => Failure {
                        legacy_status: -110,
                        committed: false,
                        message: format!("Native install generation changed: {error:?}"),
                    },
                    crate::package::scan_snapshot::CommitError::Disk { snapshot, error } => {
                        let mut message = error.message;
                        if let Some(snapshot) = snapshot {
                            reservation.mark_committed(&snapshot);
                            if let Err(publication) = environment.publish_queries(&snapshot) {
                                message.push_str(&format!(
                                    "; native query publication: {publication}"
                                ));
                            }
                        }
                        Failure {
                            legacy_status: -110,
                            committed: error.committed,
                            message,
                        }
                    }
                });
            }
        };
        reservation.mark_committed(&publication.snapshot);
        environment
            .publish_queries(&publication.snapshot)
            .map_err(Failure::after_commit)?;
        let mut packages = Vec::new();
        let mut new_installations=std::collections::BTreeSet::new();
        for metadata in publication.completed {
            let setting = &metadata.candidate.record.settings;
            for (user, state) in metadata.candidate.users {
                if state.installed {
                    if !base.owner().scanned_user_states(&setting.name).and_then(|users|users.get(&user)).is_some_and(|state|state.installed) {
                        new_installations.insert((setting.name.clone(),user as u32));
                    }
                    packages.push(InstalledPackage {
                        name: setting.name.clone(),
                        version_code: setting.version_code,
                        user: user as u32,
                    });
                }
            }
        }
        let snapshot=publication.snapshot.clone();
        Ok(NativeCommitted { snapshot, reservation, receipt: PublishedInstall {
            generation: publication.snapshot.version(),
            packages,
            verified_sessions,
            new_installations,
        } })
    }
}

#[cfg(test)]
mod publication_gate_tests {
    use super::*;
    use std::sync::{Arc,Mutex,mpsc};
    struct EnvironmentProbe{query:Arc<Mutex<Arc<crate::package::scan_snapshot::Snapshot>>>,entered:mpsc::Sender<()>,release:Mutex<mpsc::Receiver<()>>}
    impl Environment for EnvironmentProbe{
        fn reserve(&self,_:Vec<VerifiedCode>,_:&Arc<crate::package::scan_snapshot::Snapshot>)->Result<Box<dyn Reservation>,Failure>{panic!("commit fixture does not run preparation")}
        fn publish_queries(&self,snapshot:&Arc<crate::package::scan_snapshot::Snapshot>)->Result<(),String>{self.entered.send(()).unwrap();self.release.lock().unwrap().recv().unwrap();*self.query.lock().unwrap()=snapshot.clone();Ok(())}
        fn finish(&self,_:PublishedInstall)->Result<(),Exception>{panic!("commit fixture does not run completion")}
    }
    struct ReservationProbe{gate:Arc<Mutex<()>>,finished:Arc<std::sync::atomic::AtomicBool>}
    impl Reservation for ReservationProbe{
        fn requests(&self)->Result<Vec<crate::package::scan::live_install::Request>,Failure>{panic!("prepared admission fixture")}
        fn users(&self)->&[crate::package::scan::User]{&[]}
        fn build_debuggable(&self)->bool{false}
        fn complete_metadata(&self,_:crate::package::scan::live_install::Admission,_:&Apks)->Result<crate::package::scan::live_install::CompletedAdmission,Failure>{panic!("prepared admission fixture")}
        fn prepare_runtime(&self,_:&mut crate::package::scan::live_install::CompletedAdmission)->Result<(),Failure>{panic!("prepared admission fixture")}
        fn cross_user_suspensions(&self)->bool{false}
        fn mark_committed(&self,_:&Arc<crate::package::scan_snapshot::Snapshot>){assert!(self.gate.try_lock().is_err());}
        fn publication_finished(&self,_:&Arc<crate::package::scan_snapshot::Snapshot>)->Result<(),Exception>{assert!(self.gate.try_lock().is_ok());self.finished.store(true,std::sync::atomic::Ordering::Release);Ok(())}
    }
    #[test]
    fn metadata_writer_waits_for_real_keyset_commit_and_query_publication(){
        for nested in [false,true] {
        use crate::package::{scan::SigningScan,settings::{Settings,Package},owner::{Store,key_sets,usage::Usage}};
        use p256::elliptic_curve::sec1::ToEncodedPoint;
        static NEXT:std::sync::atomic::AtomicUsize=std::sync::atomic::AtomicUsize::new(0);
        let path=std::env::temp_dir().join(format!("aim-install-publish-{}-{}",std::process::id(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));
        std::fs::create_dir(&path).unwrap();
        let settings=Settings{packages:vec![Package{name:"p".into(),app_id:10100,code_path:"/data/app/p".into(),domain_set_id:Some("00000000-0000-0000-0000-000000000001".into()),..Default::default()}],..Default::default()};
        let mut owner=SigningScan::new(&Default::default(),&settings,36).unwrap();
        owner.capture_replica_runtime(std::collections::BTreeMap::from([(("p".into(),false),crate::package::scan::ReplicaRuntime{usage:[0;8],seinfo:None,override_seinfo:None,library_files:vec![],libraries:vec![]})])).unwrap();
        let snapshots=Arc::new(crate::package::scan_snapshot::Store::new(owner,Usage::new(["p"])).unwrap());let base=snapshots.capture();
        let mut disk=Store::create(&path,&[]).unwrap();disk.commit_scan_settings(&base).unwrap();let disk=Arc::new(Mutex::new(disk));
        let mut candidate=base.owner().clone();let mut scalar=[0u8;32];scalar[31]=1;
        let mut key=vec![0x30,0x59,0x30,0x13,0x06,0x07,0x2a,0x86,0x48,0xce,0x3d,0x02,0x01,0x06,0x08,0x2a,0x86,0x48,0xce,0x3d,0x03,0x01,0x07,0x03,0x42,0x00];
        key.extend_from_slice(p256::SecretKey::from_slice(&scalar).unwrap().public_key().to_encoded_point(false).as_bytes());key_sets::register(&mut candidate.settings,"p",&[key],None,&[]).unwrap();
        let query=Arc::new(Mutex::new(base.clone()));let gate=Arc::new(Mutex::new(()));let finished=Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (entered,at_projection)=mpsc::channel();let (release,resume)=mpsc::channel();
        let environment:Arc<dyn Environment>=Arc::new(EnvironmentProbe{query:query.clone(),entered,release:Mutex::new(resume)});
        let wrong=NativePrepared{publication_gate:gate.clone(),snapshots:snapshots.clone(),disk:disk.clone(),environment:environment.clone(),base:base.clone(),reservation:Box::new(ReservationProbe{gate:gate.clone(),finished:finished.clone()}),admission:crate::package::scan::live_install::CompletedAdmission{owner:candidate.clone(),completed:vec![]},verified_sessions:vec![]};
        let unrelated=Arc::new(Mutex::new(()));let wrong_permit=PublicationPermit::acquire(&unrelated);
        let error=wrong.commit_under_permit(&wrong_permit).err().expect("wrong owner permit must reject");
        assert!(!error.committed);assert_eq!(snapshots.capture().owner().settings.key_sets.last_issued_key_id,0);drop(wrong_permit);
        let prepared=NativePrepared{publication_gate:gate.clone(),snapshots:snapshots.clone(),disk:disk.clone(),environment,base:base.clone(),reservation:Box::new(ReservationProbe{gate:gate.clone(),finished:finished.clone()}),admission:crate::package::scan::live_install::CompletedAdmission{owner:candidate,completed:vec![]},verified_sessions:vec![]};
        let worker_gate=gate.clone();
        let installer=std::thread::spawn(move||{
            if nested {let permit=PublicationPermit::acquire(&worker_gate);let committed=prepared.commit_under_permit(&permit)?;drop(permit);committed.finish()}
            else {Box::new(prepared).commit()}
        });
        if at_projection.recv().is_err(){panic!("native commit failed before query publication: {:?}",installer.join().unwrap().err().map(|error|error.message));}
        assert_eq!(disk.lock().unwrap().state().settings.key_sets.last_issued_key_id,1);
        assert_eq!(snapshots.capture().owner().settings.key_sets.last_issued_key_id,1);
        assert_eq!(query.lock().unwrap().owner().settings.key_sets.last_issued_key_id,0);
        let (observed,observation)=mpsc::channel();let writer_gate=gate.clone();let writer_disk=disk.clone();let writer_query=query.clone();
        let writer=std::thread::spawn(move||{let _guard=writer_gate.lock().unwrap();let query=writer_query.lock().unwrap().clone();writer_disk.lock().unwrap().validate_committed_scan(query.owner()).unwrap();observed.send(query.owner().settings.key_sets.clone()).unwrap();});
        assert!(observation.recv_timeout(std::time::Duration::from_millis(30)).is_err());release.send(()).unwrap();
        let receipt=installer.join().unwrap().unwrap();let actual=observation.recv().unwrap();writer.join().unwrap();
        assert_eq!(actual,disk.lock().unwrap().state().settings.key_sets);
        assert_eq!(receipt.generation,snapshots.capture().version());assert!(finished.load(std::sync::atomic::Ordering::Acquire));
        drop(disk);std::fs::remove_dir_all(path).unwrap();
        }
    }
}

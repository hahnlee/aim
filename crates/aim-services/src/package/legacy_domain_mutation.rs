//! Legacy domain mutation and v1 verifier responses, AOSP android-16.0.0_r1.
use super::{domain_verification::{self, enforcer}, owner::Store};
use aim_binder_host::parcel::{Exception, EX_ILLEGAL_STATE};
use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Mutex, mpsc}, thread::{self, JoinHandle}};

pub fn set_user_state(owners: &impl enforcer::Owners, store: &mut Store,
        uid: i32, package: &str, user: i32, status: i32) -> Result<bool, Exception> {
    let allowed = enforcer::authorize(owners, uid, super::apps_filter::user_id(uid), enforcer::Operation::LegacySelect(package, user))
        .map_err(|error| match error { enforcer::Error::Security => Exception::security("legacy domain selection denied"), enforcer::Error::Owner(message) => Exception::new(EX_ILLEGAL_STATE, message) })?;
    if !allowed { return Ok(false); }
    let mut state = store.state().settings.domain_verification.clone();
    let index = match state.legacy.iter().position(|(name, _)| name.as_deref() == Some(package)) {
        Some(index) => index,
        None => { state.legacy.push((Some(package.into()), vec![])); state.legacy.len() - 1 }
    };
    let users = &mut state.legacy[index].1;
    if let Some((_, value)) = users.iter_mut().find(|(id, _)| *id == user) { *value = status; }
    else { users.push((user, status)); users.sort_by_key(|(id, _)| *id); }
    state.legacy.sort_by_key(|(name, _)| name.as_deref().map_or(0, super::info::java_hash));
    store.commit_domains(&state).map_err(|error| Exception::new(EX_ILLEGAL_STATE, format!("legacy domain persistence committed={}: {}", error.committed, error.message)))?;
    Ok(true)
}
pub struct Current { pub identifier: String, pub domains: Vec<String>, pub code_exists: bool }
pub type Lookup = Arc<dyn Fn(&str) -> Result<Option<Current>, Exception> + Send + Sync>;
pub type SetStatus = Arc<dyn Fn(i32, &str, &[String], i32) -> Result<i32, Exception> + Send + Sync>;
struct Request { identifier: String, package: String }
struct Response { id: i32, uid: i32, failed: Vec<String> }
pub struct Owner {
    requests: Mutex<BTreeMap<i32, Request>>, lookup: Lookup, set: SetStatus,
    queue: mpsc::Sender<Option<Response>>, legacy_proxy: bool, errors: Mutex<Vec<String>>,
}
pub struct Worker { owner: Arc<Owner>, thread: Option<JoinHandle<()>> }
pub struct Runtime { pub owner: Arc<Owner>, pub store: Arc<Mutex<Store>> }
impl Owner {
    pub fn start(legacy_proxy: bool, lookup: Lookup, set: SetStatus) -> (Arc<Self>, Worker) {
        let (send, receive) = mpsc::channel();
        let owner = Arc::new(Self { requests: Mutex::new(BTreeMap::new()), lookup, set, queue: send, legacy_proxy, errors: Mutex::new(vec![]) });
        let running = owner.clone();
        let thread = thread::spawn(move || { while let Ok(Some(response)) = receive.recv() {
            if let Err(error) = running.process(response) { running.errors.lock().unwrap().push(format!("legacy verifier response: {error:?}")); }
        }});
        (owner.clone(), Worker { owner, thread: Some(thread) })
    }
    /// Native domain broadcast owner registers the exact outgoing token and ID.
    pub fn register(&self, id: i32, package: String, identifier: String) {
        self.requests.lock().unwrap().retain(|_, entry| entry.package != package);
        self.requests.lock().unwrap().insert(id, Request { identifier, package });
    }
    pub fn verify(&self, id: i32, _verification_code: i32, failed: Vec<String>, uid: i32, permitted: bool) -> Result<(), Exception> {
        if !permitted { return Err(Exception::security("Only the intent filter verification agent can verify applications")); }
        // V2-only proxies do not consume the legacy message; this is the original
        // configured proxy behavior, rather than a fabricated verification result.
        if self.legacy_proxy {
            self.queue.send(Some(Response { id, uid, failed })).map_err(|_| Exception::new(EX_ILLEGAL_STATE, "legacy verification worker stopped"))?;
        }
        Ok(())
    }
    fn process(&self, response: Response) -> Result<(), Exception> {
        let request = self.requests.lock().unwrap().get(&response.id).map(|r| (r.identifier.clone(), r.package.clone()));
        let Some((identifier, package)) = request else { return Ok(()); };
        let Some(current) = (self.lookup)(&package)? else { return Ok(()); };
        if current.identifier != identifier || !current.code_exists { return Ok(()); }
        let hosts = current.domains.into_iter().collect::<BTreeSet<_>>();
        let mut failed = response.failed.into_iter().collect::<BTreeSet<_>>();
        let mut successful = hosts.difference(&failed).cloned().collect::<BTreeSet<_>>();
        for domain in successful.clone() {
            if let Some(plain) = domain.strip_prefix("*.") {
                if failed.contains(plain) {
                    failed.insert(domain.clone()); successful.remove(&domain);
                    if !hosts.contains(plain) { failed.remove(plain); }
                }
            }
        }
        for (domains, status) in [(successful, 1), (failed, 6)] {
            if domains.is_empty() { continue; }
            match (self.set)(response.uid, &identifier, &domains.into_iter().collect::<Vec<_>>(), status) {
                Ok(0) => {}
                Ok(code) => self.errors.lock().unwrap().push(format!("legacy verifier rejected domains: {code}")),
                Err(error) => self.errors.lock().unwrap().push(format!("legacy verifier domain write: {error:?}")),
            }
        }
        Ok(())
    }
    pub fn errors(&self) -> Vec<String> { self.errors.lock().unwrap().clone() }
}
impl Drop for Worker { fn drop(&mut self) { let _ = self.owner.queue.send(None); if let Some(thread) = self.thread.take() { thread.join().unwrap(); } } }

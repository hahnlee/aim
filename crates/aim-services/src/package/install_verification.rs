//! PackageVerificationState and public verifier responses, AOSP android-16.0.0_r1.
use aim_binder_host::parcel::{Exception, EX_ILLEGAL_STATE};
use std::{collections::{BTreeMap, BTreeSet}, sync::{Arc, Mutex, Condvar}, thread::{self, JoinHandle}, time::{Duration, Instant}};
#[derive(Clone, Debug)]
pub struct Verification {
    required: BTreeSet<i32>, sufficient: BTreeSet<i32>, unanswered: BTreeSet<i32>, extended: BTreeSet<i32>,
    required_complete: bool, required_passed: bool, sufficient_complete: bool, sufficient_passed: bool,
}
impl Verification {
    pub fn new(required: impl IntoIterator<Item=i32>, sufficient: impl IntoIterator<Item=i32>) -> Self {
        let required = required.into_iter().collect::<BTreeSet<_>>();
        Self { unanswered: required.clone(), required, sufficient: sufficient.into_iter().collect(), extended: BTreeSet::new(), required_complete: false, required_passed: true, sufficient_complete: false, sufficient_passed: false }
    }
    pub fn verifier(&self, uid: i32) -> bool { self.required.contains(&uid) || self.sufficient.contains(&uid) }
    pub fn extend(&mut self, uid: i32) -> bool { self.required.contains(&uid) && self.extended.insert(uid) }
    pub fn timeout_extended(&self, uid: i32) -> bool { self.extended.contains(&uid) }
    pub fn pass_required(&mut self) -> Result<(), Exception> {
        if !self.unanswered.is_empty() { return Err(Exception::new(EX_ILLEGAL_STATE, "Required verifiers still present.")); }
        self.required_complete = true; self.required_passed = true; Ok(())
    }
    pub fn respond(&mut self, uid: i32, code: i32) {
        if self.required.contains(&uid) {
            if code == 2 { self.sufficient.clear(); }
            else if code != 1 {
                self.required_passed = false; self.unanswered.clear(); self.sufficient.clear(); self.extended.clear();
            }
            self.extended.remove(&uid); self.unanswered.remove(&uid);
            if self.unanswered.is_empty() { self.required_complete = true; }
        } else if self.sufficient.contains(&uid) {
            if code == 1 { self.sufficient_complete = true; self.sufficient_passed = true; }
            self.sufficient.remove(&uid);
            if self.sufficient.is_empty() { self.sufficient_complete = true; }
        }
    }
    pub fn respond_on_timeout(&mut self, uid: i32, code: i32) {
        if !self.required.contains(&uid) { return; }
        self.sufficient.clear();
        if self.unanswered.contains(&uid) { self.respond(uid, code); }
    }
    pub fn complete(&self) -> bool { self.required_complete && (self.sufficient.is_empty() || self.sufficient_complete) }
    pub fn allowed(&self) -> bool { self.required_complete && self.required_passed && (!self.sufficient_complete || self.sufficient_passed) }
}
pub type Completion = Box<dyn FnOnce(bool) -> Result<(), Exception> + Send>;
struct Entry { verification: Verification, completed: Completion }
struct Response { id: i32, uid: i32, code: i32, at: Instant }
#[derive(Default)]
struct State { entries: BTreeMap<i32, Entry>, responses: Vec<Response>, stop: bool, errors: Vec<String> }
pub struct Owner { state: Mutex<State>, wake: Condvar }
pub struct Worker { owner: Arc<Owner>, thread: Option<JoinHandle<()>> }
impl Owner {
    pub fn start() -> (Arc<Self>, Worker) {
        let owner = Arc::new(Self { state: Mutex::new(State::default()), wake: Condvar::new() });
        let run = owner.clone(); let thread = thread::spawn(move || run.run());
        (owner.clone(), Worker { owner, thread: Some(thread) })
    }
    /// Called by the native verifying install phase before verifier broadcasts.
    pub fn register(&self, id: i32, verification: Verification, completion: Completion) -> Result<(), Exception> {
        let mut state = self.state.lock().unwrap();
        if state.stop || state.entries.contains_key(&id) { return Err(Exception::new(EX_ILLEGAL_STATE, "verification identity unavailable")); }
        state.entries.insert(id, Entry { verification, completed: completion }); Ok(())
    }
    /// Nonnegative IDs require PACKAGE_VERIFICATION_AGENT before enqueue.
    /// Negative test IDs retain original wrapping-negation semantics.
    pub fn verify(&self, id: i32, code: i32, uid: i32, permitted: bool) -> Result<(), Exception> {
        if id >= 0 && !permitted { return Err(Exception::security("Only package verification agents can verify applications")); }
        let id = if id < 0 { id.wrapping_neg() } else { id };
        let mut state = self.state.lock().unwrap();
        if state.entries.get(&id).is_some_and(|entry| entry.verification.verifier(uid)) {
            state.responses.push(Response { id, uid, code, at: Instant::now() }); self.wake.notify_one();
        }
        Ok(())
    }
    pub fn extend(&self, id: i32, code: i32, delay_millis: i64, uid: i32, permitted: bool) -> Result<(), Exception> {
        if id >= 0 && !permitted { return Err(Exception::security("Only package verification agents can extend verification timeouts")); }
        let id = if id < 0 { id.wrapping_neg() } else { id };
        let mut state = self.state.lock().unwrap();
        if state.entries.get_mut(&id).is_some_and(|entry| entry.verification.extend(uid)) {
            state.responses.push(Response { id, uid, code, at: Instant::now() + Duration::from_millis(delay_millis.clamp(0, 3_600_000) as u64) }); self.wake.notify_one();
        }
        Ok(())
    }
    pub fn stop(&self) {
        let entries = { let mut state = self.state.lock().unwrap(); state.stop = true; state.responses.clear(); std::mem::take(&mut state.entries) };
        self.wake.notify_one(); drop(entries);
    }
    pub fn errors(&self) -> Vec<String> { self.state.lock().unwrap().errors.clone() }
    fn run(&self) {
        loop {
            let completed = {
                let mut state = self.state.lock().unwrap();
                loop {
                    if state.stop { return; }
                    let now = Instant::now();
                    if let Some(index) = state.responses.iter().position(|response| response.at <= now) {
                        let response = state.responses.remove(index);
                        if let Some(entry) = state.entries.get_mut(&response.id) {
                            // Extended delayed messages are PACKAGE_VERIFIED, not timeout messages.
                            entry.verification.respond(response.uid, response.code);
                            if entry.verification.complete() {
                                let entry = state.entries.remove(&response.id).unwrap();
                                state.responses.retain(|queued| queued.id != response.id);
                                break Some((entry.verification.allowed(), entry.completed));
                            }
                        }
                        continue;
                    }
                    state = match state.responses.iter().map(|response| response.at).min() {
                        Some(at) => self.wake.wait_timeout(state, at.saturating_duration_since(now)).unwrap().0,
                        None => self.wake.wait(state).unwrap(),
                    };
                }
            };
            if let Some((allowed, complete)) = completed {
                if let Err(error) = complete(allowed) { self.state.lock().unwrap().errors.push(format!("verification install continuation: {error:?}")); }
            }
        }
    }
}
impl Drop for Worker { fn drop(&mut self) { self.owner.stop(); if let Some(thread) = self.thread.take() { thread.join().unwrap(); } } }

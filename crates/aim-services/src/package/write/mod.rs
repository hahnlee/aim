//! The write model (M4 slice B, docs/m4-packagemanager.md): what the
//! original PackageManager's writes change, computed natively without
//! side effects and compared with what the original changed.
//!
//! A write comes as the shadow's copy of its call, after the original
//! replied. Its pre-state is the latest state the model observed before
//! the driver took the call (`ShadowCall::sent`), so it never holds the
//! write itself. The model answers the call from it (its exceptions are
//! compared as any reply is) and applies what it changes to a replica of
//! the package's state in that user; further writes to the same package
//! and user apply to the replica in turn. Once none came for [`SETTLE`],
//! every write has reached the original, and the replica is compared with
//! a fresh state of it ([`Writes::checks`]).
//!
//! Installs, updates and removals are found as changes between observed
//! states ([`change`]) and compared the same way, once settled.

mod change;
mod enabled;
mod session;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::android_content_pm_ipackagemanager as pm;

use super::apps_filter::{AppsFilter, Config, NotModelled};
use super::info::user_state;
use super::model::{PackageState, State};
use super::query::{Query, States};
use crate::shadow::{Answer, Check, CheckOutcome, ShadowCall, Value};
use session::Sessions;

/// How long a package's state in a user takes no write before the
/// replica is compared with the original's.
const SETTLE: Duration = Duration::from_secs(2);
/// How long the observed states are kept, beyond the newest one older.
const HISTORY: Duration = Duration::from_secs(10);
/// How long a check waits for the feed to catch up with the original.
const FRESH: Duration = Duration::from_secs(2);

/// The writes in flight in the model, and the states they start from.
#[derive(Default)]
pub struct Writes {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The states observed and when, oldest first.
    seen: VecDeque<(Instant, Arc<State>)>,
    /// The apps filter of the latest pre-state.
    filter: Option<(Arc<State>, Arc<AppsFilter>)>,
    /// The package states being written, by package and user.
    tracks: BTreeMap<(String, i32), Track>,
    /// The packages installed, updated or removed, in the order found.
    changes: Vec<Change>,
    /// `AppIdSettingMap`'s first available app id: raised past each app
    /// id a removal frees, from system_server's start.
    first_available: i32,
    /// The install sessions the bridge tells of, once watched.
    sessions: Option<Arc<Sessions>>,
}

/// A package's install, update or removal, waiting to settle.
struct Change {
    name: String,
    kind: change::Kind,
    /// The state before it.
    pre: Arc<State>,
    found: Instant,
}

/// One package's state in one user while it is written.
struct Track {
    replica: Enabled,
    /// The calls applied: sequence number and code.
    calls: Vec<(u64, u32)>,
    /// Why the replica no longer stands for the original's state.
    not_modelled: Option<&'static str>,
    last: Instant,
}

/// What the enabled settings write of a package's state in a user.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Enabled {
    pub enabled: i32,
    pub last_disable_app_caller: Option<String>,
    pub enabled_components: BTreeSet<String>,
    pub disabled_components: BTreeSet<String>,
}

impl Enabled {
    fn of(ps: &PackageState, user: i32) -> Enabled {
        let u = user_state(ps, user);
        Enabled {
            enabled: u.enabled,
            last_disable_app_caller: u.last_disable_app_caller,
            enabled_components: u.enabled_components.into_iter().collect(),
            disabled_components: u.disabled_components.into_iter().collect(),
        }
    }

    fn value(&self) -> Value {
        let list = |s: &BTreeSet<String>| Value::List(s.iter().cloned().map(Value::Str).collect());
        Value::Fields(vec![
            ("enabled".into(), Value::Int(self.enabled)),
            (
                "lastDisableAppCaller".into(),
                self.last_disable_app_caller
                    .clone()
                    .map_or(Value::Null, Value::Str),
            ),
            ("enabledComponents".into(), list(&self.enabled_components)),
            ("disabledComponents".into(), list(&self.disabled_components)),
        ])
    }
}

impl Writes {
    /// Joins installs to the install sessions system_server's bridge
    /// tells of from now on.
    pub fn watch_sessions(&self, system: &Arc<crate::system::System>) {
        self.inner.lock().unwrap().sessions = Some(Sessions::start(system));
    }

    /// Keeps `state`, observed now, as a pre-state of the writes taken
    /// from now on.
    pub fn observe(&self, state: &Arc<State>) {
        let now = Instant::now();
        let mut inner = self.inner.lock().unwrap();
        if inner
            .seen
            .back()
            .is_some_and(|(_, s)| Arc::ptr_eq(s, state))
        {
            return;
        }
        if let Some((_, prev)) = inner.seen.back().cloned() {
            for (name, kind) in change::changes(&prev, state) {
                if !inner.changes.iter().any(|c| c.name == name) {
                    inner.changes.push(Change {
                        name,
                        kind,
                        pre: prev.clone(),
                        found: now,
                    });
                }
            }
        }
        inner.seen.push_back((now, state.clone()));
        // The newest state older than HISTORY stays: it is the pre-state
        // of any write taken since.
        while inner.seen.len() > 1 && now.duration_since(inner.seen[1].0) > HISTORY {
            inner.seen.pop_front();
        }
    }

    /// Answers a write of `package`, or `None` for another call.
    pub fn answer(&self, call: &mut ShadowCall<'_>) -> Option<Answer> {
        if call.descriptor != pm::DESCRIPTOR
            || !matches!(
                call.code,
                pm::SET_COMPONENT_ENABLED_SETTING | pm::SET_APPLICATION_ENABLED_SETTING
            )
        {
            return None;
        }
        let Some(setting) = enabled::Setting::read(call) else {
            return Some(Answer::NotModelled);
        };
        let mut inner = self.inner.lock().unwrap();
        let Some(pre) = inner.pre_state(call.sent) else {
            return Some(Answer::NotModelled);
        };
        let filter = inner.filter(&pre);
        let q = Query {
            state: &pre,
            filter: &filter,
            calling_uid: call.sender_euid as i32,
        };
        let tracks = &inner.tracks;
        let current = |ps: &PackageState, user: i32| match tracks.get(&(ps.name.clone(), user)) {
            Some(t) => t.replica.clone(),
            None => Enabled::of(ps, user),
        };
        let decided = enabled::decide(&q, &setting, call.sender_pid, &current);
        let now = Instant::now();
        let key = (setting.package, setting.user);
        let after = match decided {
            Err(NotModelled(reason)) => {
                // What the original did to the package is unknown now.
                if let Some(t) = inner.tracks.get_mut(&key) {
                    t.not_modelled.get_or_insert(reason);
                    t.calls.push((call.seq, call.code));
                    t.last = now;
                }
                return Some(Answer::NotModelled);
            }
            Ok(Err(e)) => {
                let mut p = Parcel::new();
                p.write_exception(&e);
                return Some(Answer::Reply(p));
            }
            Ok(Ok(after)) => after,
        };
        if let Some(after) = after {
            let track = inner.tracks.entry(key).or_insert(Track {
                replica: after.clone(),
                calls: Vec::new(),
                not_modelled: None,
                last: now,
            });
            track.replica = after;
            track.calls.push((call.seq, call.code));
            track.last = now;
        }
        Some(Answer::Reply(enabled::reply(call.code)))
    }

    /// The checks of the package states no write has come to for
    /// [`SETTLE`]: each replica against a fresh state of the original.
    pub fn checks(&self, states: &States) -> Vec<Check> {
        self.checks_at(states, Instant::now())
    }

    fn checks_at(&self, states: &States, now: Instant) -> Vec<Check> {
        let due: Vec<((String, i32), Track)> = {
            let mut inner = self.inner.lock().unwrap();
            let keys: Vec<(String, i32)> = inner
                .tracks
                .iter()
                .filter(|(_, t)| now.duration_since(t.last) >= SETTLE)
                .map(|(k, _)| k.clone())
                .collect();
            keys.into_iter()
                .filter_map(|k| inner.tracks.remove(&k).map(|t| (k, t)))
                .collect()
        };
        let changed: Vec<Change> = {
            let mut inner = self.inner.lock().unwrap();
            let (due, waiting) = std::mem::take(&mut inner.changes)
                .into_iter()
                .partition(|c| now.duration_since(c.found) >= SETTLE);
            inner.changes = waiting;
            due
        };
        if due.is_empty() && changed.is_empty() {
            return Vec::new();
        }
        let state = states(FRESH);
        if let Some(state) = &state {
            self.observe(state);
        }
        let mut checks: Vec<Check> = changed
            .into_iter()
            .map(|c| self.check_change(c, state.as_deref()))
            .collect();
        checks.extend(due.into_iter().map(|((package, user), track)| {
            let outcome = match (&track.not_modelled, &state) {
                (Some(reason), _) => CheckOutcome::NotModelled(reason.to_string()),
                (None, None) => CheckOutcome::NotModelled("no fresh state".into()),
                (None, Some(state)) => {
                    let original = state.packages.get(&package).map(|ps| Enabled::of(ps, user));
                    match original {
                        Some(o) if o == track.replica => CheckOutcome::Matched,
                        o => CheckOutcome::Differed {
                            original: o.map_or(Value::Null, |o| o.value()),
                            model: track.replica.value(),
                        },
                    }
                }
            };
            Check {
                service: "package".into(),
                descriptor: pm::DESCRIPTOR.into(),
                operation: "enabled settings".into(),
                calls: track.calls,
                subject: format!("{package} user {user}"),
                outcome,
            }
        }));
        checks
    }

    /// Compares a settled install, update or removal with the original's,
    /// `post` the original's state now.
    fn check_change(&self, c: Change, post: Option<&State>) -> Check {
        let outcome = match post {
            None => CheckOutcome::NotModelled("no fresh state".into()),
            Some(post) => {
                let mut inner = self.inner.lock().unwrap();
                let pkg = post.packages.get(&c.name).and_then(|ps| ps.pkg.as_deref());
                let session = match c.kind {
                    change::Kind::Install | change::Kind::Update => {
                        inner.sessions.as_ref().and_then(|s| s.installed(&c.name))
                    }
                    _ => None,
                };
                let model = change::model(
                    c.kind,
                    &c.pre,
                    post,
                    &c.name,
                    pkg,
                    session.as_ref(),
                    inner.first_available,
                );
                if c.kind == change::Kind::Removal
                    && let Some(id) = change::freed_app_id(&c.pre, post, &c.name)
                {
                    inner.first_available = inner.first_available.max(id + 1);
                }
                match model {
                    Err(NotModelled(reason)) => CheckOutcome::NotModelled(reason.into()),
                    Ok(model) => {
                        let original =
                            change::original(c.kind, &c.pre, post, &c.name, session.is_some());
                        if original == model {
                            CheckOutcome::Matched
                        } else {
                            CheckOutcome::Differed {
                                original: original.value(),
                                model: model.value(),
                            }
                        }
                    }
                }
            }
        };
        Check {
            service: "package".into(),
            descriptor: pm::DESCRIPTOR.into(),
            operation: c.kind.name().into(),
            calls: Vec::new(),
            subject: c.name,
            outcome,
        }
    }
}

impl Inner {
    /// The latest state observed before `sent`.
    fn pre_state(&self, sent: Instant) -> Option<Arc<State>> {
        self.seen
            .iter()
            .rev()
            .find(|(at, _)| *at < sent)
            .map(|(_, s)| s.clone())
    }

    fn filter(&mut self, state: &Arc<State>) -> Arc<AppsFilter> {
        match &self.filter {
            Some((s, f)) if Arc::ptr_eq(s, state) => f.clone(),
            _ => {
                let config = Config {
                    force_system_packages_queryable: state.system.force_system_packages_queryable,
                    force_queryable_packages: state.system.force_queryable_packages.clone(),
                };
                let filter = Arc::new(AppsFilter::new(state, &config));
                self.filter = Some((state.clone(), filter.clone()));
                filter
            }
        }
    }
}

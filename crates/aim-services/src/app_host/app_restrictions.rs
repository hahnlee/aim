//! AppRestrictionController read-side state, android-16.0.0_r1 (Apache-2.0).
//! Public writes and abuse trackers remain outside this owner (#2575).
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aim_android_xml::{
    Element, Value,
    pull::{Event, Reader},
};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::{AppHost, appops_service};

const API: &str = "android.app.app_restrictions_api";
const AUTO: &str = "bg_auto_restricted_bucket_on_bg_restricted";
const NOTIFICATIONS: [&str; 2] = ["last_batt_noti_ts", "last_long_fgs_noti_ts"];
const USER: i32 = 0x403; // FORCED_BY_USER / FLAG_INTERACTION
const USAGE: i32 = 0x303; // USAGE / USER_INTERACTION
const DEFAULT: i32 = 0x100;
const RUN_ANY: i32 = 70;

type Key = (i32, String);
type Result<T> = std::result::Result<T, Exception>;

fn error(e: impl std::fmt::Display) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, format!("app restrictions: {e}"))
}

fn level(bucket: i32, restricted: bool, hibernating: bool, api: bool) -> i32 {
    if hibernating {
        60
    } else if bucket == 5 {
        20
    } else if (bucket == 50 && !api) || restricted {
        50
    } else if bucket == 45 {
        40
    } else {
        30
    }
}

#[derive(Clone, PartialEq)]
struct Row {
    xml: Element,
    last: i32,
    restricted: bool,
    bucket: i32,
    pending: Option<i32>,
    active: bool,
}
impl Row {
    fn new(uid: i32, package: &str, bucket: i32, restricted: bool) -> Self {
        Self {
            xml: Element {
                name: "settings".into(),
                attrs: vec![
                    ("uid".into(), Value::Int(uid)),
                    ("package".into(), Value::String(package.into())),
                ],
                content: vec![],
            },
            last: 0,
            restricted,
            bucket,
            pending: None,
            active: false,
        }
    }
    fn int(&self, name: &str) -> i32 {
        self.xml
            .string(name)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    fn put(&mut self, name: &str, value: Value) {
        if let Some((_, v)) = self.xml.attrs.iter_mut().find(|(n, _)| n == name) {
            *v = value;
        } else {
            self.xml.attrs.push((name.into(), value));
        }
    }
    fn update(&mut self, next: i32, reason: i32) -> bool {
        let current = self.int("curlevel");
        if next == current {
            return false;
        }
        self.last = current;
        self.put("curlevel", Value::Int(next));
        self.put("reason", Value::Int(reason));
        self.put(
            "levelts",
            Value::Long(super::clock_ms(libc::CLOCK_REALTIME)),
        );
        true
    }
}

#[derive(Clone, Default, PartialEq)]
struct Settings {
    rows: BTreeMap<Key, Row>,
    auto: bool,
}
impl Settings {
    fn get(&self, uid: i32) -> i32 {
        self.rows
            .iter()
            .filter(|((u, _), _)| *u == uid)
            .fold(0, |l, (_, r)| {
                if l == 0 {
                    r.int("curlevel")
                } else {
                    l.min(r.int("curlevel"))
                }
            })
    }
    fn restore(&mut self, bytes: &[u8]) -> Result<()> {
        let mut parser = Reader::new(bytes).map_err(error)?;
        loop {
            let e = match parser.next().map_err(error)? {
                Event::EndDocument => break,
                Event::Start(e) if e.name == "settings" => e,
                _ => continue,
            };
            let uid = e.string("uid").and_then(|s| s.parse().ok()).unwrap_or(0);
            let Some(package) = e.string("package") else {
                continue;
            };
            let Some(row) = self.rows.get_mut(&(uid, package.to_string())) else {
                continue;
            };
            let int = |n: &str| e.string(n).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            // Notification metadata restores even when the saved level loses.
            for (name, value) in &e.attrs {
                if !matches!(
                    name.as_str(),
                    "uid" | "package" | "curlevel" | "reason" | "levelts"
                ) {
                    row.put(name, value.clone());
                }
            }
            if i64::from(row.int("curlevel")) < int("curlevel") {
                row.update(
                    int("curlevel") as i32,
                    e.string("reason")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(DEFAULT),
                );
                row.put("levelts", Value::Long(int("levelts")));
            }
        }
        Ok(())
    }
    fn persist(&self, data: &Path) -> Result<()> {
        // UserManager currently owns only user 0. Other users' files are untouched.
        let dir = data.join("system_de/0/apprestriction");
        std::fs::create_dir_all(dir.parent().unwrap()).map_err(error)?;
        let roots: Vec<_> = self
            .rows
            .iter()
            .filter(|((u, _), _)| *u / 100000 == 0)
            .map(|(_, r)| {
                let mut r = r.clone();
                for name in NOTIFICATIONS {
                    if r.xml.string(name).is_none() {
                        r.put(name, Value::Long(0));
                    }
                }
                r.xml
            })
            .collect();
        let bytes = aim_android_xml::abx::write_sequence(&roots).map_err(error)?;
        super::slice::write_atomic(&dir, "settings.xml", &bytes).map_err(error)
    }
}

#[derive(Default)]
pub(super) struct AppRestrictions(
    Mutex<Option<Settings>>,
    Mutex<Option<Exception>>,
    std::sync::atomic::AtomicBool,
    std::sync::atomic::AtomicBool,
);

impl AppHost {
    pub(crate) fn shutdown_restriction_persistence(&self) {
        // Wait for the current save and reject callbacks still queued by standby.
        let _settings = self.app_restrictions.0.lock().unwrap();
        self.app_restrictions
            .3
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn restriction_persistence_open(&self) -> Result<()> {
        if self
            .app_restrictions
            .3
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(error("persistence stopped"));
        }
        Ok(())
    }

    pub(super) fn restriction_locked_boot_completed(&self) {
        self.app_restrictions
            .2
            .store(true, std::sync::atomic::Ordering::Release);
    }

    fn restriction_inputs(&self, uid: i32, package: &str, startup: bool) -> Result<bool> {
        let mode = if startup {
            appops_service::restriction_mode(self, uid, package)?
        } else {
            appops_service::mode(self, RUN_ANY, uid, Some(package))?
        };
        Ok(mode != appops_service::MODE_ALLOWED)
    }

    fn restriction_apply(
        &self,
        key: &Key,
        row: &mut Row,
        next: i32,
        mut reason: i32,
        allow_bucket: bool,
        api: bool,
        auto: bool,
    ) -> Result<()> {
        let host = self.me.upgrade().ok_or_else(|| error("host gone"))?;
        let (bucket, bucket_reason) = host.restriction_standby(&key.1, key.0 / 100000)?;
        row.bucket = bucket;
        if next == row.int("curlevel") {
            return Ok(());
        }
        if level(bucket, false, false, api) == next && bucket_reason != 0 {
            reason = bucket_reason;
        }
        let old = row.int("curlevel");
        let previous_reason = row.int("reason");
        row.update(next, reason);
        if !api && (!allow_bucket || bucket == 5) {
            return Ok(());
        }
        if next >= 40 && old < 40 && bucket != 45 && (auto || next == 40) {
            if self.uid_runs(key.0) {
                row.pending = Some(reason);
            } else {
                host.restriction_standby_effect(&key.1, key.0 / 100000, None, reason)?;
            }
        } else if old >= 40 && next < 40 {
            row.pending = None;
            host.restriction_standby_effect(&key.1, key.0 / 100000, Some(previous_reason), reason)?;
        }
        row.bucket = host.restriction_standby(&key.1, key.0 / 100000)?.0;
        if row.bucket != bucket
            && (!api
                || self
                    .app_restrictions
                    .2
                    .load(std::sync::atomic::Ordering::Acquire))
        {
            // The standby owner's resulting notification is processed after the effect.
            self.restriction_apply(
                key,
                row,
                level(row.bucket, row.restricted, false, api),
                DEFAULT,
                false,
                api,
                auto,
            )?;
        }
        Ok(())
    }

    /// Initialize defaults first, restore only stricter matching rows, then save.
    pub(super) fn start_app_restrictions(self: &Arc<Self>) -> Result<()> {
        let mut guard = self.app_restrictions.0.lock().unwrap();
        self.restriction_persistence_open()?;
        if guard.is_some() {
            return Ok(());
        }
        let api = self.flag(API)?;
        let auto = self
            .platform()?
            .settings
            .device_config("activity_manager", AUTO)
            .is_some_and(|s| s.eq_ignore_ascii_case("true"));
        super::app_hibernation::AppHibernation(self.clone()).unhibernate(0)?;
        let mut settings = Settings {
            auto,
            ..Default::default()
        };
        for (package, uid, _) in self.installed(0x200, 0)? {
            let bucket = self.restriction_standby(&package, 0)?.0;
            let restricted = self.restriction_inputs(uid, &package, true)?;
            let mut row = Row::new(uid, &package, bucket, restricted);
            let key = (uid, package);
            self.restriction_apply(
                &key,
                &mut row,
                level(bucket, restricted, false, api),
                USER,
                true,
                api,
                auto,
            )?;
            row.active = self.uid_runs(uid);
            settings.rows.insert(key, row);
        }
        match std::fs::read(self.data()?.join("system_de/0/apprestriction/settings.xml")) {
            Ok(bytes) => {
                let before = settings.clone();
                settings.restore(&bytes)?;
                for (key, row) in &mut settings.rows {
                    let restored = row.clone();
                    *row = before.rows[key].clone();
                    for (name, value) in &restored.xml.attrs {
                        if !matches!(
                            name.as_str(),
                            "uid" | "package" | "curlevel" | "reason" | "levelts"
                        ) {
                            row.put(name, value.clone());
                        }
                    }
                    self.restriction_apply(
                        key,
                        row,
                        restored.int("curlevel"),
                        restored.int("reason"),
                        true,
                        api,
                        auto,
                    )?;
                    if row.int("curlevel") == restored.int("curlevel") {
                        row.put(
                            "levelts",
                            Value::Long(
                                restored
                                    .xml
                                    .string("levelts")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(0),
                            ),
                        );
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(error(e)),
        }
        settings.persist(self.data()?)?;
        *guard = Some(settings);
        Ok(())
    }

    pub(super) fn restriction_level(self: &Arc<Self>, uid: i32) -> Result<i32> {
        self.start_app_restrictions()?;
        let failure = self.app_restrictions.1.lock().unwrap();
        if let Some(e) = failure.as_ref() {
            return Err(e.clone());
        }
        Ok(self
            .app_restrictions
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .get(uid))
    }

    /// Owner notifications run after releasing their own locks. Reading latest
    /// inputs here also prevents queued notifications from replaying stale state.
    pub(super) fn restrictions_changed(&self, appops: Option<i32>) -> Result<()> {
        let mut failure = self.app_restrictions.1.lock().unwrap();
        let result = self.update_restrictions(appops);
        *failure = result.as_ref().err().cloned();
        result
    }

    fn update_restrictions(&self, appops: Option<i32>) -> Result<()> {
        let mut guard = self.app_restrictions.0.lock().unwrap();
        self.restriction_persistence_open()?;
        let Some(current) = guard.as_ref() else {
            return Ok(());
        };
        let host = self.me.upgrade().ok_or_else(|| error("host gone"))?;
        let api = self.flag(API)?;
        super::app_hibernation::AppHibernation(host.clone()).unhibernate(0)?;
        let mut next = current.clone();
        next.auto = self
            .platform()?
            .settings
            .device_config("activity_manager", AUTO)
            .is_some_and(|s| s.eq_ignore_ascii_case("true"));
        let installed = self.installed(0x200, 0)?;
        next.rows.retain(|key, _| {
            installed
                .iter()
                .any(|(p, uid, _)| key == &(*uid, p.clone()))
        });
        for (package, uid, _) in installed {
            let key = (uid, package);
            let (mut bucket, _) = host.restriction_standby(&key.1, 0)?;
            let added = !next.rows.contains_key(&key);
            let row = next
                .rows
                .entry(key.clone())
                .or_insert_with(|| Row::new(uid, &key.1, bucket, false));
            let active = self.uid_runs(uid);
            if active && !row.active && next.auto && row.int("curlevel") == 50 {
                row.pending = Some(row.int("reason"));
            }
            row.active = active;
            let mut proposed = None;
            if added || appops == Some(uid) {
                let restricted = self.restriction_inputs(uid, &key.1, added)?;
                if added {
                    proposed = Some((level(bucket, restricted, false, api), 0x600));
                } else if row.restricted != restricted {
                    let tentative = if bucket == 5 {
                        5
                    } else if row.last == 40 {
                        45
                    } else {
                        40
                    };
                    proposed = Some(if restricted {
                        (50, USER)
                    } else {
                        (level(tentative, false, false, api), USAGE)
                    });
                }
                row.restricted = restricted;
            }
            if current.auto != next.auto && row.int("curlevel") == 50 {
                host.restriction_standby_effect(
                    &key.1,
                    0,
                    (!next.auto).then_some(row.int("reason")),
                    if next.auto { row.int("reason") } else { 0x306 },
                )?;
                bucket = host.restriction_standby(&key.1, 0)?.0;
            }
            if proposed.is_none()
                && row.bucket != bucket
                && (!api
                    || self
                        .app_restrictions
                        .2
                        .load(std::sync::atomic::Ordering::Acquire))
            {
                proposed = Some((level(bucket, row.restricted, false, api), DEFAULT));
            }
            if let Some((level, reason)) = proposed {
                self.restriction_apply(
                    &key,
                    row,
                    level,
                    reason,
                    appops.is_some() || added,
                    api,
                    next.auto,
                )?;
            }
            if api
                && !self
                    .app_restrictions
                    .2
                    .load(std::sync::atomic::Ordering::Acquire)
            {
                row.bucket = bucket;
            }
            if !active && let Some(reason) = row.pending.take() {
                host.restriction_standby_effect(&key.1, 0, None, reason)?;
                let bucket = host.restriction_standby(&key.1, 0)?.0;
                if row.bucket != bucket {
                    self.restriction_apply(
                        &key,
                        row,
                        level(bucket, row.restricted, false, api),
                        DEFAULT,
                        false,
                        api,
                        next.auto,
                    )?;
                }
            }
        }
        if next == *current {
            return Ok(());
        }
        next.persist(self.data()?)?;
        *guard = Some(next);
        Ok(())
    }
}

#[cfg(test)]
mod tests;

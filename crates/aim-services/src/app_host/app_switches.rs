//! App-switch state, following ATMS.LocalService at android-16.0.0_r1
//! (AOSP, Apache-2.0). Allowances are retained but never consumed by BAL.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};

use super::{AppHost, PER_USER_RANGE};

#[derive(Default)]
pub(super) struct Policy {
    pub(super) allowances: HashMap<i32, HashMap<String, i32>>,
    stopped: Option<Instant>,
}

impl Policy {
    pub(super) fn set(&mut self, kind: String, uid: i32, user: i32, running: bool) {
        if !running {
            return;
        }
        if uid < 0 {
            if let Some(types) = self.allowances.get_mut(&user) {
                types.remove(&kind);
            }
        } else {
            self.allowances.entry(user).or_default().insert(kind, uid);
        }
    }

    pub(super) fn clear_allowances(&mut self) {
        self.allowances.clear();
    }

    pub(super) fn switches(&mut self, stop: bool) {
        self.stopped = stop.then(Instant::now);
    }

    pub(super) fn allows(
        &self,
        _uid: i32,
        foreground: bool,
        privileged: bool,
        now: Instant,
    ) -> bool {
        privileged
            || foreground
                && self
                    .stopped
                    .is_none_or(|stop| now.duration_since(stop) >= Duration::from_millis(500))
    }
}

impl AppHost {
    pub(super) fn background_activity_start_allowed(&self, uid: i32) -> Result<bool, Exception> {
        let foreground = self.uid_runs(uid);
        if self
            .tasks
            .app_switches
            .lock()
            .unwrap()
            .allows(uid, foreground, false, Instant::now())
        {
            return Ok(true);
        }
        Ok(uid == 0
            || matches!(uid % PER_USER_RANGE as i32, 1000 | 1027)
            || self.check_permission(
                Some("android.permission.START_ACTIVITIES_FROM_BACKGROUND"),
                uid,
            )? == 0)
    }

    pub(super) fn app_switches_reply(
        &self,
        descriptor: &str,
        stop: bool,
        call: &mut Call<'_>,
    ) -> Reply {
        call.data
            .enforce_interface(descriptor)
            .map_err(|_| BAD_VALUE)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        super::reply(
            self.set_app_switches(call.sender_euid as i32, stop)
                .map(|()| {
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    p
                }),
        )
    }

    pub(super) fn set_app_switches(&self, uid: i32, stop: bool) -> Result<(), Exception> {
        if !matches!(uid, 0 | 1000)
            && self.check_permission(Some("android.permission.STOP_APP_SWITCHES"), uid)? != 0
        {
            return Err(Exception::security(
                "requires android.permission.STOP_APP_SWITCHES",
            ));
        }
        self.tasks.app_switches.lock().unwrap().switches(stop);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowances_store_replace_remove_and_clear_without_launch_effect() {
        let mut p = Policy::default();
        p.set("service".into(), 10123, 0, true);
        p.set("service".into(), 110123, 1, true);
        p.set("service".into(), 10124, 0, true);
        p.set("other".into(), 10124, 0, true);
        assert_eq!(p.allowances[&0]["service"], 10124);
        assert_eq!(p.allowances[&1]["service"], 110123);
        p.set("service".into(), -1, 0, true);
        assert_eq!(p.allowances[&0].len(), 1);
        p.set("stopped".into(), 210123, 2, false);
        assert!(!p.allowances.contains_key(&2));
        p.allowances.remove(&0); // ATMS.LocalService.onUserStopped.
        assert_eq!(p.allowances.len(), 1);
        p.switches(true);
        let now = p.stopped.unwrap();
        assert!(!p.allows(110123, false, false, now));
        assert!(!p.allows(110123, true, false, now));
        assert!(p.allows(110123, true, false, now + Duration::from_millis(500)));
        assert!(p.allows(110123, false, true, now));
        p.switches(false);
        assert!(p.allows(110123, true, false, now));
        p.clear_allowances();
        assert!(p.allowances.is_empty());
    }
}

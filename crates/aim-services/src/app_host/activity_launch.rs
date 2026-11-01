//! ActivityStarter.waitResultIfNeeded / ActivityTaskSupervisor, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{AppHost, activity::Activities, activity_start::StartArgs};
use aim_binder_host::local::Call;
use aim_binder_host::parcel::{Exception, Parcel};

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct WaitResult {
    pub result: i32,
    pub timeout: bool,
    pub who: Option<(String, String)>,
    pub total_time: i64,
    pub launch_state: i32,
}
impl WaitResult {
    pub fn reply(&self) -> Parcel {
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(1);
        p.write_i32(self.result);
        p.write_bool(self.timeout);
        if let Some((package, class)) = &self.who {
            p.write_string16(Some(package));
            p.write_string16(Some(class));
        } else {
            p.write_string16(None);
        }
        p.write_i64(self.total_time);
        p.write_i32(self.launch_state);
        p
    }
    pub fn shell_output(&self, wait_ms: u128) -> String {
        let state = match self.launch_state {
            1 => "COLD".into(),
            2 => "WARM".into(),
            3 => "HOT".into(),
            4 => "RELAUNCH".into(),
            n => format!("UNKNOWN ({n})"),
        };
        let mut text = format!(
            "Status: {}\nLaunchState: {state}\n",
            if self.timeout { "timeout" } else { "ok" }
        );
        if let Some((package, class)) = &self.who {
            text.push_str(&format!(
                "Activity: {}\n",
                super::services::short_name(package, class)
            ));
        }
        if self.total_time >= 0 {
            text.push_str(&format!("TotalTime: {}\n", self.total_time));
        }
        text.push_str(&format!("WaitTime: {wait_ms}\nComplete"));
        text
    }
}
#[derive(Default)]
struct State {
    token: Option<u64>,
    result: WaitResult,
    done: bool,
}
pub(super) struct Launch {
    started: Instant,
    state: Mutex<State>,
    ready: Condvar,
}
impl Launch {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
        })
    }
    fn wait(&self, code: i32, timeout: Duration) -> WaitResult {
        let mut state = self.state.lock().unwrap();
        if state.token.is_some() && matches!(code, 0 | 2) {
            let (s, timed) = self
                .ready
                .wait_timeout_while(state, timeout.saturating_sub(self.started.elapsed()), |s| {
                    !s.done
                })
                .unwrap();
            state = s;
            if timed.timed_out() && !state.done {
                state.result.timeout = true;
                state.result.total_time = -1;
                state.result.launch_state = -1;
                state.done = true;
            }
        }
        if state.result.result == 0 {
            state.result.result = code;
        }
        state.result.clone()
    }
    fn finish(&self, token: u64, completion: Option<(bool, i64, i32)>) {
        let mut s = self.state.lock().unwrap();
        if !s.done && s.token == Some(token) {
            if let Some((timeout, time, kind)) = completion {
                s.result.timeout = timeout;
                s.result.total_time = time;
                s.result.launch_state = kind;
            } else {
                s.result.total_time = self.started.elapsed().as_millis() as i64;
            }
            s.done = true;
            self.ready.notify_all();
        }
    }
}
pub(super) struct Tracking<'a> {
    pub source: Option<u64>,
    pub kind: i32,
    pub waiter: Option<&'a Arc<Launch>>,
}
impl Activities {
    pub(super) fn finish_launch(&self, token: u64, completion: Option<(bool, i64, i32)>) {
        for launch in self.launches.lock().unwrap().iter() {
            launch.finish(token, completion);
        }
    }
}
impl AppHost {
    pub(super) fn track_activity_launch(&self, token: u64, code: i32, tracking: Tracking<'_>) {
        let mut launches = self.activities.launches.lock().unwrap();
        let (who, drawn) = {
            let records = self.activities.records.lock().unwrap();
            let Some(r) = records.get(&token) else { return };
            let real = super::activity_start::real_component(&r.info);
            let (package, class) = real.split_once('/').unwrap();
            ((package.to_string(), class.to_string()), r.drawn)
        };
        let stack = self.activities.stack.lock().unwrap();
        let visible = stack
            .get(token)
            .is_some_and(|a| a.state == super::activity_stack::State::Resumed && stack.shown(a));
        drop(stack);
        if let Some(waiter) = tracking.waiter {
            waiter.state.lock().unwrap().token = Some(token);
            launches.push(waiter.clone());
        }
        for launch in launches.iter() {
            let mut s = launch.state.lock().unwrap();
            if s.done
                || !(s.token == Some(token)
                    || (tracking.source.is_some() && s.token == tracking.source))
            {
                continue;
            }
            s.token = Some(token);
            s.result.who = Some(who.clone());
            s.result.launch_state = tracking.kind;
            if matches!(code, 2 | 3) {
                s.result.result = code;
            }
            if code == 3 || (code == 2 && drawn && visible) {
                s.result.total_time = 0;
                s.result.launch_state = 0;
                s.done = true;
                launch.ready.notify_all();
            }
        }
    }
    pub(super) fn activity_drawn(&self, token: u64) {
        let launches = self.activities.launches.lock().unwrap();
        let mut records = self.activities.records.lock().unwrap();
        let Some(r) = records.get_mut(&token) else {
            return;
        };
        r.drawn = true;
        let relaunched = r.relaunched;
        r.relaunched = false;
        drop(records);
        for launch in launches.iter() {
            if relaunched {
                let mut s = launch.state.lock().unwrap();
                if !s.done && s.token == Some(token) && s.result.launch_state == 3 {
                    s.result.launch_state = 4;
                }
            }
            launch.finish(token, None);
        }
    }
    pub(super) fn wait_activity_start(
        self: &Arc<Self>,
        call: &Call<'_>,
        mut args: StartArgs,
        user: i32,
        unsupported: Option<&str>,
    ) -> Result<WaitResult, Exception> {
        let launch = Launch::new();
        args.launch = Some(launch.clone());
        let result = self
            .start_activity(call, args, Some(user), unsupported, None)
            .map(|code| launch.wait(code, Duration::from_secs(10)));
        self.activities
            .launches
            .lock()
            .unwrap()
            .retain(|l| !Arc::ptr_eq(l, &launch));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::super::kept::RawIntent;
    use super::super::{
        activity_stack::Decision,
        activity_start::StartRequest,
        activity_start_tests::{permissions, with_start},
    };
    use super::*;
    use aim_binder_host::{
        local::{Reply, Service},
        parcel::{Binder, EX_SECURITY},
    };
    use aim_service_aidl::android_app_iactivitytaskmanager as atm;

    fn source(host: &Arc<AppHost>) -> u64 {
        use super::super::{
            activity_stack::{Activity, Task},
            activity_start::LaunchIntent,
        };
        let info = host.activity_info_of("p", "p.Main").unwrap();
        let token = host.new_activity(
            1,
            10100,
            "p",
            info,
            Default::default(),
            7,
            StartRequest::launcher("p.Main"),
        );
        let mut stack = host.activities.stack.lock().unwrap();
        stack.tasks.push(Task {
            id: 7,
            affinity: None,
            real_activity: "p/p.Main".into(),
            intent: Default::default(),
            base: LaunchIntent::Component,
            uid: 10100,
            user: 0,
            last_active: 0,
        });
        stack
            .activities
            .push(Activity::new(token, 7, 1, "p/p.Main", 0, 0, None));
        token
    }
    // Pinned ActivityStarter/WaitResult: admitted cold launch, per-activity draw,
    // hot reuse, configuration relaunch, cleanup and idle-timeout semantics.
    #[test]
    fn launch_wait_tracks_draw_reuse_relaunch_cancellation_and_timeout() {
        with_start(|host, _| {
            permissions(host);
            let info = host.activity_info_of("p", "p.Main").unwrap();
            source(host);
            let cold = Launch::new();
            let token = host
                .start_in_process(
                    0,
                    10100,
                    "p",
                    info,
                    StartRequest {
                        task: Some(7),
                        ..StartRequest::launcher("p.Main")
                    },
                    &Decision::default(),
                    Some(Tracking {
                        source: None,
                        kind: 1,
                        waiter: Some(&cold),
                    }),
                )
                .unwrap();
            host.activity_drawn(token + 1);
            assert!(!cold.state.lock().unwrap().done);
            host.activity_drawn(token);
            let result = cold.wait(0, Duration::ZERO);
            let status = (result.result, result.timeout, result.launch_state);
            assert_eq!(status, (0, false, 1));
            assert_eq!(result.who, Some(("p".into(), "p.Main".into())));
            for (relaunched, expected) in [(false, 3), (true, 4)] {
                let launch = Launch::new();
                host.activities.with(Some(Binder::Local(token)), |r| {
                    r.drawn = false;
                    r.relaunched = relaunched;
                });
                host.track_activity_launch(
                    token,
                    2,
                    Tracking {
                        source: None,
                        kind: 3,
                        waiter: Some(&launch),
                    },
                );
                assert!(!launch.state.lock().unwrap().done);
                host.activity_drawn(token);
                let result = launch.wait(2, Duration::ZERO);
                assert_eq!(
                    (result.result, result.launch_state, result.timeout),
                    (2, expected, false)
                );
            }
            let top = Launch::new();
            host.track_activity_launch(
                token,
                3,
                Tracking {
                    source: None,
                    kind: 3,
                    waiter: Some(&top),
                },
            );
            let result = top.wait(3, Duration::ZERO);
            let status = (result.result, result.total_time, result.launch_state);
            assert_eq!(status, (3, 0, 0));
            for cancelled in [false, true] {
                let launch = Launch::new();
                host.activities
                    .with(Some(Binder::Local(token)), |r| r.drawn = false);
                host.track_activity_launch(
                    token,
                    0,
                    Tracking {
                        source: None,
                        kind: 1,
                        waiter: Some(&launch),
                    },
                );
                if cancelled {
                    host.activities.remove_start(0, true);
                }
                let result = launch.wait(0, Duration::ZERO);
                assert_eq!(
                    (result.timeout, result.total_time, result.launch_state),
                    (!cancelled, -1, if cancelled { 0 } else { -1 })
                );
            }
        });
    }
    struct Thread;
    impl Service for Thread {
        fn descriptor(&self) -> &str {
            "android.app.IApplicationThread"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            Ok(Parcel::new())
        }
    }
    #[test]
    fn wait_entry_validates_callers_aborts_and_returns_warm_first_draw() {
        with_start(|host, export| {
            permissions(host);
            let token = source(host);
            let request = |uid, package: &str, user, caller, flags| {
                let mut p = Parcel::new();
                let mut a = super::super::activity_start_tests::args(
                    Some(package),
                    user,
                    Some(RawIntent::of(None, 0, &[]).with_component("p", "p.Main")),
                );
                a.caller = caller;
                a.flags = flags;
                a.result_to = Some(Binder::Local(token));
                a.write(&mut p);
                super::super::activity::ActivityTaskManager(host.clone(), Binder::Local(0))
                    .transact(&mut Call {
                        code: atm::START_ACTIVITY_AND_WAIT,
                        flags: 0,
                        sender_pid: 71,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap()
            };
            for (uid, package, user) in [(10100, "q", 0), (10100, "p", 10), (99000, "p", 0)] {
                assert_eq!(
                    request(uid, package, user, None, 0)
                        .reader()
                        .read_exception()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    EX_SECURITY
                );
            }
            for (uid, caller, flags, expected) in [
                (1000, None, 2, -96),
                (1000, Some(Binder::Handle(u32::MAX)), 0, -94),
                (10100, None, 0, 102),
            ] {
                let p = request(
                    uid,
                    if uid == 1000 { "android" } else { "p" },
                    0,
                    caller,
                    flags,
                );
                let mut r = p.reader();
                r.read_exception().unwrap().unwrap();
                assert_eq!(r.read_i32().unwrap(), 1);
                assert_eq!(r.read_i32().unwrap(), expected);
                assert!(!r.read_bool().unwrap());
                assert_eq!(r.read_string16().unwrap(), None);
                assert_eq!(r.read_i64().unwrap(), 0);
                assert_eq!(r.read_i32().unwrap(), 0);
            }
            let fixture = super::super::tests::host_with_start(1, Arc::new(|| {}));
            let mut app = fixture.apps.lock().unwrap().remove(&1).unwrap();
            app.package = "p".into();
            app.process = "p".into();
            app.uid = 10100;
            app.package_uid = 10100;
            app.isolated = false;
            app.thread = Some(export(Arc::new(Thread)));
            app.config = Some(host.global_configuration().unwrap());
            host.apps.lock().unwrap().insert(1, app);
            std::thread::scope(|scope| {
                let child = scope.spawn(|| request(1000, "android", 0, None, 0));
                let deadline = Instant::now() + Duration::from_secs(5);
                let token = loop {
                    let token = host
                        .activities
                        .launches
                        .lock()
                        .unwrap()
                        .first()
                        .and_then(|l| l.state.lock().unwrap().token);
                    if let Some(token) = token {
                        break token;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::yield_now();
                };
                host.activity_drawn(token);
                let p = child.join().unwrap();
                let mut r = p.reader();
                r.read_exception().unwrap().unwrap();
                assert_eq!(r.read_i32().unwrap(), 1);
                assert_eq!(r.read_i32().unwrap(), 0);
                assert!(!r.read_bool().unwrap());
                assert_eq!(r.read_string16().unwrap().as_deref(), Some("p"));
                assert_eq!(r.read_string16().unwrap().as_deref(), Some("p.Main"));
                assert!(r.read_i64().unwrap() >= 0);
                assert_eq!(r.read_i32().unwrap(), 2);
            });
            assert!(host.activities.launches.lock().unwrap().is_empty());
        });
    }
    #[test]
    fn shell_wait_output_matches_android_format() {
        let mut w = WaitResult {
            who: Some(("p".into(), "p.Main".into())),
            total_time: 23,
            launch_state: 1,
            ..Default::default()
        };
        assert_eq!(
            w.shell_output(25),
            "Status: ok\nLaunchState: COLD\nActivity: p/.Main\nTotalTime: 23\nWaitTime: 25\nComplete"
        );
        w.timeout = true;
        w.total_time = -1;
        w.launch_state = -1;
        assert_eq!(
            w.shell_output(10000),
            "Status: timeout\nLaunchState: UNKNOWN (-1)\nActivity: p/.Main\nWaitTime: 10000\nComplete"
        );
    }
}

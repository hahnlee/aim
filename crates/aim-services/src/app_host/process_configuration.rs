//! A process's configuration, as `WindowProcessController` keeps it at
//! `android-16.0.0_r1`: the global configuration, overridden by that of
//! the activity it launched last and still has
//! (`updateActivityConfigurationListener`) unless it is a system process,
//! and sent to it at a new sequence number whenever it changes
//! (`onConfigurationChanged`, `dispatchConfiguration`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use super::activity::{ConfigurationChangeItem, ProcessItems};
use super::config::Configuration;
use super::{App, AppHost, PER_USER_RANGE, SYSTEM_UID};

/// `WindowConfiguration.ACTIVITY_TYPE_UNDEFINED`.
const ACTIVITY_TYPE_UNDEFINED: i32 = 0;

/// `mIsActivityConfigOverrideAllowed`: not for a system-owned process.
fn follows_activities(uid: u32) -> bool {
    uid % PER_USER_RANGE != SYSTEM_UID
}

/// `resolveOverrideConfiguration`: the configuration of a process on
/// `global` that follows an activity of `activity`, without its activity
/// type and at the global sequence number.
fn of_activity(global: &Configuration, activity: Option<&Configuration>) -> Configuration {
    let Some(activity) = activity else {
        return global.clone();
    };
    let mut config = activity.clone();
    config.window.activity_type = ACTIVITY_TYPE_UNDEFINED;
    config.seq = global.seq;
    config
}

/// `Configuration.equals`, which leaves the sequence number out.
fn same(a: &Configuration, b: &Configuration) -> bool {
    Configuration {
        seq: b.seq,
        ..a.clone()
    } == *b
}

impl AppHost {
    /// `ActivityTaskManagerService.increaseConfigurationSeqLocked`.
    pub(super) fn increase_configuration_seq(&self) -> i32 {
        (self.config_seq.fetch_add(1, Ordering::Relaxed) + 1) as i32
    }

    /// The configuration of the activity each process follows, by its
    /// start: the one it launched last of those it still has.
    fn followed_activities(&self) -> HashMap<i64, Configuration> {
        let mut followed: HashMap<i64, (i32, Configuration)> = HashMap::new();
        for r in self.activities.records.lock().unwrap().values() {
            let Some(config) = &r.reported else {
                continue;
            };
            if followed
                .get(&r.start_seq)
                .is_none_or(|(ident, _)| *ident < r.ident)
            {
                followed.insert(r.start_seq, (r.ident, config.clone()));
            }
        }
        followed.into_iter().map(|(s, (_, c))| (s, c)).collect()
    }

    /// `updateConfiguration` of `app`, on its global configuration and
    /// following an activity of `followed`: its configuration, and whether
    /// it is no longer the one it was last sent, which it then takes at a
    /// new sequence number.
    fn update_process_configuration(
        &self,
        app: &mut App,
        global: &Configuration,
        followed: Option<&Configuration>,
    ) -> (Configuration, bool) {
        let mut config = of_activity(global, followed.filter(|_| follows_activities(app.uid)));
        match &app.reported_config {
            Some(old) if same(old, &config) => (old.clone(), false),
            _ => {
                config.seq = self.increase_configuration_seq();
                app.reported_config = Some(config.clone());
                (config, true)
            }
        }
    }

    /// `prepareConfigurationForLaunchingActivity` of the process of
    /// `start_seq` on `global`, its launching activity recorded: what
    /// `LaunchActivityItem` carries.
    pub(super) fn launching_process_configuration(
        &self,
        start_seq: i64,
        global: &Configuration,
    ) -> Configuration {
        let _update = self.process_configuration_update.lock().unwrap();
        let followed = self.followed_activities().remove(&start_seq);
        let mut apps = self.apps.lock().unwrap();
        match apps.get_mut(&start_seq) {
            Some(app) => {
                self.update_process_configuration(app, global, followed.as_ref())
                    .0
            }
            None => global.clone(),
        }
    }

    /// `onConfigurationChanged` of each bound process: a
    /// `ConfigurationChangeItem` to each whose configuration is no longer
    /// the one it was last sent, at a new sequence number.
    pub(super) fn process_configuration_changes(&self) -> ProcessItems {
        let _update = self.process_configuration_update.lock().unwrap();
        let followed = self.followed_activities();
        let mut items: ProcessItems = Vec::new();
        let mut apps = self.apps.lock().unwrap();
        for (&start_seq, app) in apps.iter_mut().filter(|(_, a)| a.thread.is_some()) {
            let Some(global) = app.config.clone() else {
                continue;
            };
            let (config, changed) =
                self.update_process_configuration(app, &global, followed.get(&start_seq));
            if changed {
                items.push((start_seq, Box::new(ConfigurationChangeItem { config })));
            }
        }
        items
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{Binder, Parcel};
    use aim_service_aidl::ReadParcelable;
    use aim_service_aidl::android_app_iapplicationthread as thread;

    use super::super::activity::{Item, Launch};
    use super::super::activity_start::Report;
    use super::super::config::Rect;
    use super::*;

    const CONFIGURATION_CHANGE: &str = "android.app.servertransaction.ConfigurationChangeItem";

    /// Of a transaction, the configuration of a leading
    /// `ConfigurationChangeItem` and the class of the item after it, or the
    /// class of its first item.
    pub(in super::super) type Sent = (Option<Configuration>, Option<String>);

    /// An `IApplicationThread` keeping what it is sent.
    #[derive(Default)]
    pub(in super::super) struct Recorder(Mutex<Vec<Sent>>, Condvar);

    impl Recorder {
        /// The `n` transactions it was sent since last asked, once they
        /// arrive: they are one-way.
        pub(in super::super) fn take(&self, n: usize) -> Vec<Sent> {
            let (mut sent, timeout) = self
                .1
                .wait_timeout_while(self.0.lock().unwrap(), Duration::from_secs(2), |s| {
                    s.len() < n
                })
                .unwrap();
            assert!(!timeout.timed_out(), "{sent:?}");
            std::mem::take(&mut *sent)
        }
    }

    impl Service for Recorder {
        fn descriptor(&self) -> &str {
            thread::DESCRIPTOR
        }
        fn transact(&self, c: &mut Call<'_>) -> Reply {
            if c.code == thread::SCHEDULE_TRANSACTION {
                c.data.enforce_interface(thread::DESCRIPTOR)?;
                assert_eq!(c.data.read_i32()?, 1);
                let count = c.data.read_i32()?;
                let first = c.data.read_string16()?;
                let sent = if first.as_deref() == Some(CONFIGURATION_CHANGE) {
                    assert_eq!(c.data.read_i32()?, 1);
                    let config = Configuration::read_from(&mut c.data)?;
                    c.data.read_i32()?;
                    let next = if count > 1 {
                        c.data.read_string16()?
                    } else {
                        None
                    };
                    (Some(config), next)
                } else {
                    (None, first)
                };
                self.0.lock().unwrap().push(sent);
                self.1.notify_all();
            }
            Ok(Parcel::new())
        }
    }

    /// The process configuration a `LaunchActivityItem` carries.
    fn launched(items: &[Box<dyn Item>]) -> Configuration {
        assert_eq!(
            items[0].class(),
            "android.app.servertransaction.LaunchActivityItem"
        );
        let mut p = Parcel::new();
        items[0].write_fields(&mut p);
        let mut r = p.reader();
        r.read_binder().unwrap();
        r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1);
        Configuration::read_from(&mut r).unwrap()
    }

    #[test]
    fn a_process_follows_the_activity_it_launched_last() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let permissions = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
            host.display_metrics
                .set(Ok(super::super::display::Metrics {
                    width: 1080,
                    height: 1920,
                    density_dpi: 160,
                }))
                .unwrap();
            let window = Rect {
                left: 100,
                top: 100,
                right: 924,
                bottom: 1300,
            };
            host.tasks.windows.set(true).unwrap();
            host.tasks.animation_test_task(1, window);
            let global = host.global_configuration().unwrap();
            let recorders = [(); 2].map(|_| Arc::new(Recorder::default()));
            for ((start_seq, uid), recorder) in
                [(7, 10100), (8, SYSTEM_UID)].into_iter().zip(&recorders)
            {
                let mut app = super::super::tests::host_with_start(start_seq, Arc::new(|| {}))
                    .apps
                    .lock()
                    .unwrap()
                    .remove(&start_seq)
                    .unwrap();
                app.uid = uid;
                app.thread = Some(export(recorder.clone()));
                app.config = Some(global.clone());
                app.reported_config = Some(global.clone());
                host.apps.lock().unwrap().insert(start_seq, app);
            }
            let launch = |start_seq, uid, task, bounds| {
                let token = host.new_activity(
                    start_seq,
                    uid,
                    "p",
                    host.activity_info_of("p", "p.Main").unwrap(),
                    Default::default(),
                    task,
                    super::super::activity_start::StartRequest::launcher("p.Main"),
                );
                let items = host
                    .launch_items(
                        Binder::Local(token),
                        Launch {
                            global: &global,
                            task: bounds,
                            to: super::super::activity::LaunchTo::Resumed,
                        },
                    )
                    .unwrap();
                (token, launched(&items))
            };
            // In a freeform task the application's resources take the
            // task window's size, not the display's.
            let (_, freeform) = launch(7, 10100, 1, Some((5, window)));
            assert_eq!(
                (freeform.screen_width_dp, freeform.screen_height_dp),
                (824, 1200)
            );
            assert_eq!(freeform.window.bounds, window);
            assert_eq!(freeform.window.windowing_mode, 5);
            assert_eq!(freeform.window.activity_type, ACTIVITY_TYPE_UNDEFINED);
            assert!(freeform.seq > global.seq, "the client applies only newer");
            assert_eq!(
                host.apps.lock().unwrap()[&7].reported_config,
                Some(freeform.clone())
            );
            assert!(host.process_configuration_changes().is_empty());
            // A later activity in a fullscreen task takes it over.
            let (later, fullscreen) = launch(7, 10100, 2, None);
            assert_eq!(fullscreen.screen_width_dp, global.screen_width_dp);
            assert!(fullscreen.seq > freeform.seq);
            // An earlier launch that gets here late keeps the later one.
            assert_eq!(host.launching_process_configuration(7, &global), fullscreen);
            // Once it is gone the process follows the earlier one again.
            host.activity_reported(Some(Binder::Local(later)), Report::Destroyed);
            let to_app = recorders[0].take(1);
            let [(Some(back), None)] = &to_app[..] else {
                panic!("{to_app:?}");
            };
            assert!(same(back, &freeform));
            assert!(back.seq > fullscreen.seq);
            // A system process keeps the global configuration.
            let (_, system) = launch(8, SYSTEM_UID, 3, Some((5, window)));
            assert!(same(&system, &global));
            assert!(host.process_configuration_changes().is_empty());
            // A global change reaches it merged with that activity's.
            let mut delta = Configuration::from_proto(&[]).unwrap();
            delta.font_scale = 1.25;
            host.apply_configuration(&delta, false).unwrap();
            let global = host.global_configuration().unwrap();
            let to_app = recorders[0].take(2);
            let (Some(merged), _) = &to_app[0] else {
                panic!("{to_app:?}");
            };
            assert_eq!(merged.font_scale, 1.25);
            assert_eq!(merged.window.bounds, window);
            assert_eq!(merged.screen_width_dp, 824);
            assert!(merged.seq > back.seq);
            // A system process takes the global one.
            let to_system = recorders[1].take(2);
            assert!(same(to_system[0].0.as_ref().unwrap(), &global));
        });
    }
}

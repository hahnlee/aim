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
use super::{AppHost, PER_USER_RANGE, SYSTEM_UID};

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

    /// `prepareConfigurationForLaunchingActivity` of the process of
    /// `start_seq` on `global` once it follows its launching activity of
    /// `activity`: what `LaunchActivityItem` carries, at a new sequence
    /// number (and so the one it was last sent) if it changed.
    pub(super) fn launching_process_configuration(
        &self,
        start_seq: i64,
        global: &Configuration,
        activity: &Configuration,
    ) -> Configuration {
        let mut apps = self.apps.lock().unwrap();
        let Some(app) = apps
            .get_mut(&start_seq)
            .filter(|a| follows_activities(a.uid))
        else {
            return global.clone();
        };
        let mut config = of_activity(global, Some(activity));
        if app
            .reported_config
            .as_ref()
            .is_none_or(|old| !same(old, &config))
        {
            config.seq = self.increase_configuration_seq();
            app.reported_config = Some(config.clone());
        }
        config
    }

    /// `onConfigurationChanged` of each bound process: a
    /// `ConfigurationChangeItem` to each whose configuration is no longer
    /// the one it was last sent, at a new sequence number.
    pub(super) fn process_configuration_changes(&self) -> ProcessItems {
        let followed = self.followed_activities();
        let mut items: ProcessItems = Vec::new();
        let mut apps = self.apps.lock().unwrap();
        for (&start_seq, app) in apps.iter_mut().filter(|(_, a)| a.thread.is_some()) {
            let Some(global) = &app.config else {
                continue;
            };
            let activity = followed
                .get(&start_seq)
                .filter(|_| follows_activities(app.uid));
            let mut config = of_activity(global, activity);
            if app
                .reported_config
                .as_ref()
                .is_some_and(|old| same(old, &config))
            {
                continue;
            }
            config.seq = self.increase_configuration_seq();
            app.reported_config = Some(config.clone());
            items.push((start_seq, Box::new(ConfigurationChangeItem { config })));
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::Parcel;
    use aim_service_aidl::ReadParcelable;

    use super::super::activity::{Item, Launch};
    use super::super::config::Rect;
    use super::*;

    struct Thread;
    impl Service for Thread {
        fn descriptor(&self) -> &str {
            "android.app.IApplicationThread"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
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

    /// `ConfigurationChangeItem`'s configuration.
    fn changed(item: &dyn Item) -> Configuration {
        let mut p = Parcel::new();
        item.write_fields(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 1);
        Configuration::read_from(&mut r).unwrap()
    }

    #[test]
    fn a_process_follows_the_activity_it_launched_last() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            host.display_metrics
                .set(Ok(super::super::display::Metrics {
                    width: 1080,
                    height: 1920,
                    density_dpi: 160,
                }))
                .unwrap();
            let global = host.global_configuration().unwrap();
            for (start_seq, uid) in [(7, 10100), (8, SYSTEM_UID)] {
                let mut app = super::super::tests::host_with_start(start_seq, Arc::new(|| {}))
                    .apps
                    .lock()
                    .unwrap()
                    .remove(&start_seq)
                    .unwrap();
                app.uid = uid;
                app.thread = Some(export(Arc::new(Thread)));
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
                        aim_binder_host::parcel::Binder::Local(token),
                        Launch {
                            global: &global,
                            task: bounds,
                        },
                    )
                    .unwrap();
                (token, launched(&items))
            };
            let window = Rect {
                left: 100,
                top: 100,
                right: 924,
                bottom: 1300,
            };
            // In a freeform task the application's resources take the
            // task window's size, not the display's.
            let (_, freeform) = launch(7, 10100, 1, Some(window));
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
            // Once it is gone the process follows the earlier one again.
            host.activities.records.lock().unwrap().remove(&later);
            let items = host.process_configuration_changes();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].0, 7);
            let back = changed(items[0].1.as_ref());
            assert!(same(&back, &freeform));
            assert!(back.seq > fullscreen.seq);
            // A system process keeps the global configuration.
            let (_, system) = launch(8, SYSTEM_UID, 3, Some(window));
            assert_eq!(system, global);
            assert!(host.process_configuration_changes().is_empty());
        });
    }
}

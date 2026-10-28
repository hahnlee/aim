//! Local ActivityManager pull data at android-16.0.0_r1 (#2274).
//! Cold aggregation and event formatting stay in Java; native owners supply inventories.
//! AppProfiler/PendingIntentController semantics: AOSP android-16.0.0_r1, Apache-2.0.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iactivitystatshost as api;
use aim_service_aidl::dev_aim_server_iactivitystatssnapshot as snapshot;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::{AppHost, SYSTEM_UID, clock_ms, reply};
use crate::system::ServiceOwner;

// ProcessMemoryState/AppProtoEnums at the pinned image (DEX-checked in tests).
pub(super) const HOSTING_SYSTEM: i32 = 1;
pub(super) const HOSTING_PERSISTENT: i32 = 2;
pub(super) const HOSTING_ACTIVITY: i32 = 16;
pub(super) const HOSTING_RECEIVER: i32 = 32;
pub(super) const HOSTING_PROVIDER: i32 = 64;
pub(super) const HOSTING_STARTED_SERVICE: i32 = 128;
pub(super) const HOSTING_FOREGROUND_SERVICE: i32 = 256;
pub(super) const HOSTING_BOUND_SERVICE: i32 = 512;

#[derive(Clone, Default, Debug, PartialEq)]
pub(super) struct Watermark([i32; 12]);
impl WriteParcelable for Watermark {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(52);
        for value in self.0 {
            p.write_i32(value);
        }
    }
}
impl ReadParcelable for Watermark {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        if size < 52 || size as usize - 4 > r.remaining() {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let mut values = [0; 12];
        for v in &mut values {
            *v = r.read_i32()?;
        }
        r.set_position(start + size as usize);
        Ok(Self(values))
    }
}

#[derive(Default)]
pub(super) struct ProcessStats {
    watermark: Mutex<Watermark>,
    history: Mutex<HashMap<i64, i32>>,
    collector: Mutex<Option<Arc<ServiceOwner>>>,
}
impl ProcessStats {
    // AppProfiler.CachedAppsWatermarkData: strictly greater, and reset every field.
    fn observe(
        &self,
        count: i32,
        uptime_ms: i64,
        sample: impl FnOnce() -> Result<Watermark, Exception>,
    ) -> Result<(), Exception> {
        let mut mark = self.watermark.lock().unwrap();
        if count > mark.0[0] {
            let mut next = sample()?;
            next.0[0] = count;
            next.0[1] = (uptime_ms / 1000) as i32;
            *mark = next;
        }
        Ok(())
    }
    fn pull(&self, reset: bool) -> Watermark {
        let mut mark = self.watermark.lock().unwrap();
        if reset {
            std::mem::take(&mut *mark)
        } else {
            mark.clone()
        }
    }
}

pub(super) struct ProcessState {
    uid: i32,
    pid: i32,
    name: String,
    state: i32,
    foreground: bool,
    oom_adj: i32,
    hosting: i32,
    historical: i32,
}
impl WriteParcelable for ProcessState {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.uid);
        p.write_i32(self.pid);
        p.write_string16(Some(&self.name));
        p.write_i32(self.oom_adj);
        p.write_i32(self.state);
        p.write_bool(self.foreground);
        p.write_i32(self.hosting);
        p.write_i32(self.historical);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

impl AppHost {
    pub(super) fn note_stats_component(&self, seq: i64, component: i32) {
        *self
            .process_stats
            .history
            .lock()
            .unwrap()
            .entry(seq)
            .or_default() |= component;
    }
    pub(super) fn forget_stats_process(&self, seq: i64) {
        self.process_stats.history.lock().unwrap().remove(&seq);
    }

    fn stats_processes(&self) -> Vec<Option<ProcessState>> {
        // Do not nest the services lock inside apps (ActiveServices takes the other order).
        let services = self.services.stats_components();
        let receivers = self.broadcasts.stats_running();
        let activities: std::collections::HashSet<_> = self
            .activities
            .records
            .lock()
            .unwrap()
            .values()
            .map(|a| a.start_seq)
            .collect();
        let apps = self.apps.lock().unwrap();
        let history = self.process_stats.history.lock().unwrap();
        let mut rows: Vec<_> = apps
            .iter()
            .filter(|(_, a)| a.pid > 0)
            .map(|(seq, a)| {
                let hosting = services.get(seq).copied().unwrap_or(0)
                    | if a.process == super::SYSTEM_PROCESS {
                        HOSTING_SYSTEM
                    } else {
                        0
                    }
                    | if a.persistent { HOSTING_PERSISTENT } else { 0 }
                    | if activities.contains(seq) {
                        HOSTING_ACTIVITY
                    } else {
                        0
                    }
                    | if receivers.contains(seq) {
                        HOSTING_RECEIVER
                    } else {
                        0
                    }
                    | if !a.providers.is_empty() {
                        HOSTING_PROVIDER
                    } else {
                        0
                    };
                ProcessState {
                    uid: a.uid as i32,
                    pid: a.pid,
                    name: a.process.clone(),
                    state: a.proc_state,
                    foreground: hosting & HOSTING_FOREGROUND_SERVICE != 0,
                    oom_adj: a.oom_adj,
                    hosting,
                    historical: hosting | history.get(seq).copied().unwrap_or(0),
                }
            })
            .collect();
        rows.sort_by_key(|r| r.pid);
        rows.into_iter().map(Some).collect()
    }

    pub(super) fn observe_cached_apps(&self) -> Result<(), Exception> {
        let (count, uids) = {
            let apps = self.apps.lock().unwrap();
            let count = apps
                .values()
                .filter(|a| (16..=19).contains(&a.proc_state))
                .count();
            let mut uids: Vec<_> = apps
                .values()
                .filter(|a| a.pid > 0)
                .map(|a| a.uid as i32)
                .collect();
            uids.sort_unstable();
            uids.dedup();
            (count as i32, uids)
        };
        self.process_stats
            .observe(count, clock_ms(libc::CLOCK_UPTIME_RAW), || {
                let collector = self
                    .process_stats
                    .collector
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "cached-app snapshot owner missing",
                        )
                    })?;
                let mut p = Parcel::new();
                snapshot::Snapshot {
                    live_uids: Some(uids),
                }
                .write(&mut p);
                let response = collector
                    .transact_as_owner(snapshot::SNAPSHOT, &p, false)
                    .map_err(|s| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("cached-app snapshot: {s}"),
                        )
                    })?;
                snapshot::read_snapshot_reply::<Watermark>(&mut response.reader())
                    .map_err(|s| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("cached-app snapshot reply: {s}"),
                        )
                    })??
                    .ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "cached-app snapshot",
                        )
                    })
            })
    }

    pub(super) fn activity_stats_host(
        self: &Arc<Self>,
        collector: Option<Binder>,
    ) -> Result<Binder, Exception> {
        let collector = match collector {
            Some(Binder::Handle(h)) => ServiceOwner::Remote(Arc::new(self.process.strong(h))),
            Some(Binder::Local(ptr)) => {
                ServiceOwner::Local(self.process.local_service(ptr).ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "cached-app snapshot gone",
                    )
                })?)
            }
            None => {
                return Err(Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "cached-app snapshot",
                ));
            }
        };
        *self.process_stats.collector.lock().unwrap() = Some(Arc::new(collector));
        Ok(self.process.add_service(Arc::new(Backend(self.clone()))))
    }
}

pub(super) struct Backend(pub Arc<AppHost>);
impl Service for Backend {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "activity stats require system UID",
            )));
        }
        let mut p = Parcel::new();
        match call.code {
            api::GET_PROCESSES => {
                api::GetProcesses::read(&mut call.data)?;
                api::write_get_processes_reply(&mut p, Some(&self.0.stats_processes()));
            }
            api::GET_PENDING_INTENTS => {
                api::GetPendingIntents::read(&mut call.data)?;
                let rows = match self.0.pending_intents.stats() {
                    Ok(rows) => rows,
                    Err(status) => {
                        return reply(Err(Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            format!("PendingIntent extras: {status}"),
                        )));
                    }
                };
                api::write_get_pending_intents_reply(&mut p, Some(&rows));
            }
            api::GET_CACHED_APPS_WATERMARK => {
                let a = api::GetCachedAppsWatermark::read(&mut call.data)?;
                api::write_get_cached_apps_watermark_reply(
                    &mut p,
                    Some(&self.0.process_stats.pull(a.reset_after_pull)),
                );
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_watermark_samples_only_new_peaks_and_resets_all_fields() {
        let stats = ProcessStats::default();
        stats.observe(0, 5000, || panic!("no cached apps")).unwrap();
        assert_eq!(stats.pull(true), Watermark::default());
        stats.observe(3, 12345, || Ok(Watermark([7; 12]))).unwrap();
        let mut expected = [7; 12];
        expected[0] = 3;
        expected[1] = 12;
        for n in [3, 1] {
            stats
                .observe(n, 20000, || panic!("not a new peak"))
                .unwrap();
        }
        assert_eq!(stats.pull(false), Watermark(expected));
        assert_eq!(stats.pull(true), Watermark(expected));
        assert_eq!(stats.pull(false), Watermark::default());
        stats.observe(1, 23456, || Ok(Watermark([9; 12]))).unwrap();
        assert_eq!(&stats.pull(true).0[..3], &[1, 23, 9]);
    }
    #[test]
    #[ignore = "requires pinned original framework resources; no guest boot"]
    fn pinned_device_pinner_resources() {
        let image = aim_paths::original_image();
        let resources = super::super::resources::Framework::load(&image, 320, "en-US")
            .expect("NOT RUN: pinned framework resources unavailable");
        assert_eq!(
            resources.framework_bool("config_pinnerCameraApp"),
            Some(false)
        );
        assert_eq!(
            resources.framework_bool("config_pinnerAssistantApp"),
            Some(false)
        );
        assert_eq!(
            resources.framework_integer("config_pinnerHomePinBytes"),
            Some(62914560)
        );
        assert_eq!(
            resources.framework_string_array("config_defaultPinnerServiceFiles"),
            Some(vec![])
        );
        use aim_android_image::{
            dex::{Dex, Value},
            system_server::dex_files,
        };
        let jar = std::fs::read(image.join("system/framework/framework.jar")).unwrap();
        let mut checked = false;
        for bytes in dex_files(&jar).unwrap() {
            let dex = Dex::parse(&bytes).unwrap();
            if let Some(class) = dex.class("Landroid/app/ProcessMemoryState;") {
                let fields: std::collections::HashMap<_, _> =
                    dex.static_values(class).unwrap().into_iter().collect();
                for (name, value) in [
                    ("SYSTEM", HOSTING_SYSTEM),
                    ("PERSISTENT", HOSTING_PERSISTENT),
                    ("ACTIVITY", HOSTING_ACTIVITY),
                    ("BROADCAST_RECEIVER", HOSTING_RECEIVER),
                    ("PROVIDER", HOSTING_PROVIDER),
                    ("STARTED_SERVICE", HOSTING_STARTED_SERVICE),
                    ("FOREGROUND_SERVICE", HOSTING_FOREGROUND_SERVICE),
                    ("BOUND_SERVICE", HOSTING_BOUND_SERVICE),
                ] {
                    assert_eq!(
                        fields[&format!("HOSTING_COMPONENT_TYPE_{name}")],
                        Value::Int(value as i64)
                    );
                }
                checked = true;
            }
        }
        assert!(checked);
    }
    #[test]
    fn cached_accounting_uses_live_process_states_and_the_java_snapshot() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Collector(Arc<AtomicUsize>);
        impl Service for Collector {
            fn descriptor(&self) -> &str {
                snapshot::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                let a = snapshot::Snapshot::read(&mut call.data)?;
                assert_eq!(a.live_uids, Some(vec![99000]));
                self.0.fetch_add(1, Ordering::Relaxed);
                let mut p = Parcel::new();
                snapshot::write_snapshot_reply(&mut p, Some(&Watermark([17; 12])));
                Ok(p)
            }
        }
        let host = super::super::tests::host_with_start(7, Arc::new(|| {}));
        let samples = Arc::new(AtomicUsize::new(0));
        let collector = host
            .process
            .add_service(Arc::new(Collector(samples.clone())));
        host.activity_stats_host(Some(collector)).unwrap();
        host.observe_cached_apps().unwrap();
        assert_eq!(samples.load(Ordering::Relaxed), 0);
        {
            let mut apps = host.apps.lock().unwrap();
            let app = apps.get_mut(&7).unwrap();
            app.proc_state = 16;
            app.oom_adj = 900;
        }
        host.observe_cached_apps().unwrap();
        host.observe_cached_apps().unwrap();
        assert_eq!(samples.load(Ordering::Relaxed), 1);
        let row = host.stats_processes().pop().unwrap().unwrap();
        assert_eq!((row.state, row.oom_adj), (16, 900));
        let backend = Backend(host.clone());
        let mut request = Parcel::new();
        api::GetCachedAppsWatermark {
            reset_after_pull: true,
        }
        .write(&mut request);
        let p = backend
            .transact(&mut Call {
                code: api::GET_CACHED_APPS_WATERMARK,
                flags: 0,
                sender_pid: 1,
                sender_euid: SYSTEM_UID,
                data: request.reader(),
            })
            .unwrap();
        let mark = api::read_get_cached_apps_watermark_reply::<Watermark>(&mut p.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(mark.0[0], 1);
        assert_eq!(mark.0[2], 17);
        assert_eq!(host.process_stats.pull(false), Watermark::default());
        host.observe_cached_apps().unwrap();
        assert_eq!(samples.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn bridge_checks_uid_and_reads_the_live_process_owner() {
        let host = super::super::tests::host_with_start(7, Arc::new(|| {}));
        let backend = Backend(host.clone());
        let mut request = Parcel::new();
        api::GetProcesses {}.write(&mut request);
        let invoke = |uid| {
            backend
                .transact(&mut Call {
                    code: api::GET_PROCESSES,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: uid,
                    data: request.reader(),
                })
                .unwrap()
        };
        for code in [
            api::GET_PROCESSES,
            api::GET_PENDING_INTENTS,
            api::GET_CACHED_APPS_WATERMARK,
        ] {
            let p = backend
                .transact(&mut Call {
                    code,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 10123,
                    data: request.reader(),
                })
                .unwrap();
            assert_eq!(
                p.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
        }
        let rows = host.stats_processes();
        let row = rows[0].as_ref().unwrap();
        assert_eq!((row.uid, row.pid, row.state), (99000, 4321, 20));
        let p = invoke(SYSTEM_UID);
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 1); // array length
        assert_eq!(r.read_i32().unwrap(), 1); // typed presence
        let start = r.position();
        let size = r.read_i32().unwrap();
        assert_eq!(
            (r.read_i32().unwrap(), r.read_i32().unwrap()),
            (99000, 4321)
        );
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("WebViewLoader-arm64-v8a")
        );
        let values: Vec<_> = (0..5).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(values, [0, 20, 0, 0, 0]);
        assert_eq!(r.position() - start, size as usize);
        assert_eq!(r.remaining(), 0);
        host.apps.lock().unwrap().clear();
        let p = invoke(SYSTEM_UID);
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
    }
}

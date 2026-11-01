//! AppIdleHistory read/write/duration contracts, android-16.0.0_r1.
use super::*;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(super::super::test_temp_dir("standby-persistence"))
    }
    fn history(&self, now: i64) -> History {
        History::new(&self.0, now)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn controller(history: History) -> Controller {
    Controller {
        policy: Arc::new(Policy {
            enabled: true,
            headless: BTreeSet::new(),
        }),
        constants: Constants::read(&|_| None, true),
        history,
        screen_time_bypass: false,
    }
}

#[test]
fn original_xml_round_trip_preserves_fields_and_filters_expiries() {
    let dir = Temp::new();
    let mut h = dir.history(100);
    let name = "a<&\"\té";
    let u = h.usage(name, 0);
    *u = read_app_idle_times(
        br#"<packages version="1"><package name="old"
        elapsedIdleTime="1" lastUsedByUserElapsedTime="2" screenIdleTime="3"
        lastPredictedTime="4" appLimitBucket="45" bucketReason="60a"
        lastJobRunTime="5" lastRestrictionAttemptElapsedTime="6"
        lastRestrictionAttemptReason="602" nextEstimatedAppLaunchTime="123456">
        <expiryTimes><item bucket="10" expiry="99"/><item bucket="20" expiry="100"/>
        <item bucket="30" expiry="101"/><item bucket="40" expiry="0"/></expiryTimes>
        </package></packages>"#,
    )
    .unwrap()
    .remove("old")
    .unwrap();
    u.last_predicted_bucket = 30; // Deliberately not persisted by Android.
    let mut expected = u.clone();
    expected.last_predicted_bucket = STANDBY_BUCKET_UNKNOWN;
    expected.expiry.remove(&10);
    h.usage("untouched", 10);
    h.flush(200).unwrap();
    let file = h.user_file(0);
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("<?xml version='1.0'"));
    assert!(text.contains("<packages version=\"1\">"));
    assert!(text.contains("bucketReason=\"60a\""));
    assert!(text.contains("a&lt;&amp;&quot;&#9;é"));
    let before = std::fs::read(h.user_file(10)).unwrap();
    let mut reopened = dir.history(50);
    assert_eq!(reopened.usage(name, 0), &expected);
    reopened.usage("new", 0);
    reopened.flush(50).unwrap();
    assert_eq!(std::fs::read(h.user_file(10)).unwrap(), before);
    let legacy = read_app_idle_times(
        br#"<packages><package name="p" elapsedIdleTime="7"
        screenIdleTime="8" activeTimeoutTime="0" workingSetTimeoutTime="99"/></packages>"#,
    )
    .unwrap();
    assert_eq!(legacy["p"].expiry, BTreeMap::from([(20, 99)]));
    assert_eq!(legacy["p"].last_used_by_user_elapsed, 7);
    let legacy = write_app_idle_times(&legacy, 0);
    assert!(!legacy.contains("activeTimeoutTime"));
    assert!(!legacy.contains("lastJobRunTime"));
    assert!(!legacy.contains("nextEstimatedAppLaunchTime"));
    assert_eq!(
        read_app_idle_times(legacy.as_bytes()).unwrap()["p"].expiry[&20],
        99
    );
}

#[test]
fn duration_snapshots_follow_display_transitions_and_exclude_downtime() {
    let dir = Temp::new();
    let mut h = dir.history(100);
    h.update_display(true, 100);
    h.update_display(false, 300);
    h.flush(400).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.0.join("screen_on_time")).unwrap(),
        "200\n300\n"
    );
    h.update_display(true, 500);
    h.flush(600).unwrap();
    h.flush(700).unwrap();
    assert_eq!((h.screen_on_time(700), h.elapsed_time(700)), (400, 600));
    // A flush while on doesn't snapshot the still-on portion of screen time.
    let mut h = dir.history(20);
    assert_eq!((h.screen_on_time(20), h.elapsed_time(20)), (200, 600));
    h.update_display(true, 20);
    assert_eq!((h.screen_on_time(70), h.elapsed_time(70)), (250, 650));
    h.update_display(false, 100);
    h.flush(120).unwrap();
    let h = dir.history(9999);
    assert_eq!((h.screen_on_time(9999), h.elapsed_time(9999)), (280, 700));
}

#[test]
fn scheduled_and_started_sync_holds_extend_and_expire_after_reopening() {
    let dir = Temp::new();
    let mut c = controller(dir.history(1000));
    c.constants.sync_scheduled_timeout = 1000;
    c.constants.sync_start_timeout = 100;
    c.constants.sync_unexempted_timeout = 500;
    c.sync_usage("scheduled", 0, false, false, 1100);
    let plain = c.history.usage("scheduled", 0).clone();
    assert_eq!(plain.expiry, BTreeMap::from([(20, 600)]));
    assert_eq!((plain.current_bucket, plain.bucketing_reason), (20, 0x30e));
    c.sync_usage("active", 0, true, false, 1100);
    c.sync_usage("active", 0, true, true, 1200);
    assert_eq!(c.history.usage("active", 0).expiry[&10], 1100);
    c.sync_usage("active", 0, true, false, 1300);
    c.sync_usage("active", 0, true, true, 1400);
    assert_eq!(c.history.usage("active", 0).expiry[&10], 1300);
    c.sync_usage("started", 0, true, true, 1400);
    c.history.set_bucket("restricted", 0, 1400, 45, 0x602);
    let r = c.history.usage("restricted", 0);
    r.last_restrict_attempt_elapsed = 400;
    r.last_restrict_reason = 0x602;
    let restricted = r.clone();
    c.sync_usage("restricted", 0, true, false, 1401);
    c.sync_usage("restricted", 0, true, true, 1402);
    c.history.flush(1450).unwrap();
    let mut c = controller(dir.history(10)); // Elapsed duration now 450.
    assert_eq!(c.history.usage("scheduled", 0), &plain);
    assert_eq!(c.sync_usage("scheduled", 0, false, false, 10), None);
    assert_eq!(c.history.usage("restricted", 0), &restricted);
    let u = c.history.usage("active", 0);
    assert_eq!((u.current_bucket, u.bucketing_reason), (10, 0x30d));
    assert_eq!(
        (
            u.last_used_elapsed,
            u.last_used_screen,
            u.last_used_by_user_elapsed
        ),
        (i32::MIN as i64, i32::MIN as i64, i32::MIN as i64)
    );
    for (p, expiry, bucket) in [
        ("started", 500, 10),
        ("scheduled", 600, 20),
        ("active", 1300, 10),
    ] {
        let at = 10 + expiry - 450;
        AppHost::check_and_update(&mut c, p, 0, 50, at - 1);
        assert_eq!(c.history.bucket(p, 0), bucket);
        AppHost::check_and_update(&mut c, p, 0, 50, at);
        assert_eq!(c.history.bucket(p, 0), 50);
    }
}

#[test]
fn write_faults_keep_committed_files_and_reload_recovers_atomic_artifacts() {
    let dir = Temp::new();
    let mut h = dir.history(100);
    h.set_bucket("p", 0, 100, 20, 0x100);
    h.flush(200).unwrap();
    let file = h.user_file(0);
    let new = file.with_file_name("app_idle_stats.xml.new");
    std::fs::create_dir(&new).unwrap(); // startWrite fails, old base survives.
    h.set_bucket("p", 0, 200, 10, 0x30b);
    assert!(h.flush(300).is_err());
    std::fs::remove_dir(&new).unwrap();
    let mut reopened = dir.history(10);
    assert_eq!(reopened.bucket("p", 0), 20);
    assert_eq!(reopened.elapsed_time(10), 200); // Independent duration commit.
    let duration_new = dir.0.join("screen_on_time.new");
    std::fs::create_dir(&duration_new).unwrap();
    assert!(h.flush(400).is_err());
    std::fs::remove_dir(&duration_new).unwrap();
    let mut reopened = dir.history(10);
    assert_eq!(reopened.bucket("p", 0), 10); // User file still commits.
    assert_eq!(reopened.elapsed_time(10), 200);
    h.flush(500).unwrap(); // Retry must not count elapsed duration twice.
    let committed = std::fs::read(&file).unwrap();
    let bak = file.with_file_name("app_idle_stats.xml.bak");
    std::fs::rename(&file, &bak).unwrap();
    std::fs::write(&file, b"broken base").unwrap();
    std::fs::write(&new, b"interrupted partial write").unwrap();
    let duration = dir.0.join("screen_on_time");
    std::fs::rename(&duration, dir.0.join("screen_on_time.bak")).unwrap();
    std::fs::write(&duration, b"broken base").unwrap();
    std::fs::write(&duration_new, b"partial").unwrap();
    let mut reopened = dir.history(20);
    assert_eq!(reopened.elapsed_time(20), 400);
    assert_eq!(reopened.bucket("p", 0), 10);
    assert_eq!(std::fs::read(file).unwrap(), committed);
    assert!(!bak.exists() && !new.exists() && !duration_new.exists());
    // finishWrite fails after the new bytes have been synced: cleanup removes
    // the new file and leaves the existing destination untouched.
    let target = reopened.user_file(10);
    mkdirs(&target).unwrap();
    std::fs::write(target.join("sentinel"), b"keep").unwrap();
    assert!(write_history_file(&target, b"replacement").is_err());
    assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"keep");
    assert!(!target.with_file_name("app_idle_stats.xml.new").exists());
    assert_eq!(dir.history(20).bucket("p", 10), STANDBY_BUCKET_NEVER);
}

#[test]
fn disabled_apps_and_disabled_policy_survive_host_flush_and_shutdown() {
    crate::system::with_discovery_binders(|host, _, enabled, _| {
        enabled(None, 2); // PackageManager.COMPONENT_ENABLED_STATE_DISABLED.
        let (state, resolution) = host.packages().unwrap();
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let info = query
            .application_info("p", MATCH_DISABLED_COMPONENTS, 0)
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!info.enabled);
        assert!(
            host.installed(MATCH_DISABLED_COMPONENTS, 0)
                .unwrap()
                .iter()
                .any(|(p, _, _)| p == "p")
        );
        let dir = host.data().unwrap().join("system");
        let now = elapsed_realtime();
        let mut c = controller(History::new(&dir, now));
        c.sync_usage("p", 0, true, false, now);
        *host.app_standby.controller.lock().unwrap() = Some(c);
        host.flush_standby().unwrap();
        let mut reopened = History::new(&dir, elapsed_realtime());
        assert_eq!(reopened.bucket("p", 0), 10);
        let before = reopened.usage("p", 0).clone();
        assert!(!host.app_idle_enabled().unwrap()); // Fixture auto-power-modes disabled.
        assert_eq!(host.app_standby_bucket("p", 0).unwrap(), 5);
        host.report_sync_usage("p", 0, true, true).unwrap();
        host.with_standby(|_, c| {
            c.sync_usage("q", 0, true, true, elapsed_realtime());
            Ok(())
        })
        .unwrap();
        host.shutdown_persistence().unwrap();
        let mut reopened = History::new(&dir, elapsed_realtime());
        assert_eq!(reopened.usage("p", 0), &before);
        assert_eq!(reopened.usage("q", 0).bucketing_reason, 0x30d);
        let file = reopened.user_file(0);
        std::fs::write(&file, b"closed").unwrap();
        host.flush_standby().unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"closed");
        assert!(host.with_standby(|_, _| Ok(())).is_err());
    });
}

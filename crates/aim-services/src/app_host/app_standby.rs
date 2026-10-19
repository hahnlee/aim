//! App standby in the app host: AppStandbyController and AppIdleHistory
//! (`apex/jobscheduler/service/java/com/android/server/usage`,
//! `android-16.0.0_r1`), the apps' standby buckets that `usagestats`
//! answers.
//!
//! The history is user 0's as the original persisted it
//! (`/data/system/users/<user>/app_idle_stats.xml`, its time bases in
//! `/data/system/screen_on_time`), read when first needed. As the original
//! does once its services are ready and its boot completed, every
//! installed app's bucket is checked (`checkIdleStates`) against its
//! lowest bucket (`getAppMinBucket`) and how long ago it was used, and
//! again every `mCheckIdleIntervalMillis`; an activity resumed or paused
//! holds its app active for the strong usage timeout, and its bucket is
//! checked again when that ends. The display, the Mac's, is on.
//!
//! What the app host does not do: write the history back, other usage
//! events (notifications seen, foreground services, sync, slices, content
//! providers, user interaction), predictions and restrictions, cross-profile
//! apps, and the lowest bucket of an app a device admin or the active
//! network scorer exempts (#1723).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::settings::Table;
use super::{AppHost, PER_USER_RANGE};
use crate::package::query::Query;

/// `UsageStatsManager.STANDBY_BUCKET_*`.
const STANDBY_BUCKET_EXEMPTED: i32 = 5;
const STANDBY_BUCKET_ACTIVE: i32 = 10;
const STANDBY_BUCKET_WORKING_SET: i32 = 20;
const STANDBY_BUCKET_FREQUENT: i32 = 30;
const STANDBY_BUCKET_RARE: i32 = 40;
/// `AppIdleHistory.IDLE_BUCKET_CUTOFF`.
const IDLE_BUCKET_CUTOFF: i32 = STANDBY_BUCKET_RARE;
const STANDBY_BUCKET_RESTRICTED: i32 = 45;
const STANDBY_BUCKET_NEVER: i32 = 50;
/// `AppIdleHistory.STANDBY_BUCKET_UNKNOWN`.
const STANDBY_BUCKET_UNKNOWN: i32 = -1;
/// `UsageStatsManager.REASON_*`.
const REASON_MAIN_MASK: i32 = 0xFF00;
const REASON_MAIN_DEFAULT: i32 = 0x0100;
const REASON_MAIN_TIMEOUT: i32 = 0x0200;
const REASON_MAIN_USAGE: i32 = 0x0300;
const REASON_MAIN_FORCED_BY_USER: i32 = 0x0400;
const REASON_MAIN_PREDICTED: i32 = 0x0500;
const REASON_SUB_MASK: i32 = 0x00FF;
const REASON_SUB_DEFAULT_APP_RESTORED: i32 = 0x0002;
const REASON_SUB_USAGE_SYSTEM_UPDATE: i32 = 0x0006;
const REASON_SUB_USAGE_MOVE_TO_FOREGROUND: i32 = 0x0004;
const REASON_SUB_USAGE_MOVE_TO_BACKGROUND: i32 = 0x0005;
const REASON_SUB_USAGE_USER_INTERACTION: i32 = 0x0003;
const REASON_SUB_USAGE_ACTIVE_TIMEOUT: i32 = 0x0007;
const REASON_SUB_PREDICTED_RESTORED: i32 = 0x0001;
const FIRST_APPLICATION_UID: i32 = 10_000;
const SYSTEM_UID: i32 = 1000;
/// `ApplicationInfo.FLAG_SYSTEM`; `PackageManager.MATCH_DISABLED_COMPONENTS`,
/// `MATCH_SYSTEM_ONLY`, `MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`,
/// `MATCH_ANY_USER`.
const FLAG_SYSTEM: i32 = 0x1;
const MATCH_DISABLED_COMPONENTS: i64 = 0x200;
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
const MATCH_DIRECT_BOOT_AWARE_UNAWARE: i64 = 0x0004_0000 | 0x0008_0000;
const MATCH_ANY_USER: i64 = 0x0040_0000;
const ACCESS_BACKGROUND_LOCATION: &str = "android.permission.ACCESS_BACKGROUND_LOCATION";
const USE_EXACT_ALARM: &str = "android.permission.USE_EXACT_ALARM";
const SCHEDULE_EXACT_ALARM: &str = "android.permission.SCHEDULE_EXACT_ALARM";
/// `AlarmManager.ENABLE_USE_EXACT_ALARM`, `SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT`,
/// `SCHEDULE_EXACT_ALARM_DOES_NOT_ELEVATE_BUCKET`, `REQUIRE_EXACT_ALARM_PERMISSION`.
const ENABLE_USE_EXACT_ALARM: i64 = 218_533_173;
const SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT: i64 = 226_439_802;
const SCHEDULE_EXACT_ALARM_DOES_NOT_ELEVATE_BUCKET: i64 = 262_645_982;
const REQUIRE_EXACT_ALARM_PERMISSION: i64 = 171_306_433;

const ONE_MINUTE: i64 = 60_000;
const ONE_HOUR: i64 = 60 * ONE_MINUTE;
const ONE_DAY: i64 = 24 * ONE_HOUR;
/// AppStandbyController's thresholds, their minimums and the buckets they
/// lead to.
const DEFAULT_SCREEN_TIME_THRESHOLDS: [i64; 5] = [0, 0, ONE_HOUR, 2 * ONE_HOUR, 6 * ONE_HOUR];
const MINIMUM_SCREEN_TIME_THRESHOLDS: [i64; 5] = [0, 0, 0, 30 * ONE_MINUTE, ONE_HOUR];
const DEFAULT_ELAPSED_TIME_THRESHOLDS: [i64; 5] =
    [0, 12 * ONE_HOUR, 24 * ONE_HOUR, 48 * ONE_HOUR, 8 * ONE_DAY];
const MINIMUM_ELAPSED_TIME_THRESHOLDS: [i64; 5] =
    [0, ONE_HOUR, ONE_HOUR, 2 * ONE_HOUR, 4 * ONE_HOUR];
const THRESHOLD_BUCKETS: [i32; 5] = [
    STANDBY_BUCKET_ACTIVE,
    STANDBY_BUCKET_WORKING_SET,
    STANDBY_BUCKET_FREQUENT,
    STANDBY_BUCKET_RARE,
    STANDBY_BUCKET_RESTRICTED,
];
const BUCKET_NAMES: [&str; 5] = ["active", "working_set", "frequent", "rare", "restricted"];
/// `ConstantsObserver`'s defaults.
const DEFAULT_CHECK_IDLE_INTERVAL_MS: i64 = 4 * ONE_HOUR;
const DEFAULT_LEGACY_STRONG_USAGE_TIMEOUT: i64 = ONE_HOUR;
const DEFAULT_CURRENT_STRONG_USAGE_TIMEOUT: i64 = 5 * ONE_MINUTE;
const DEFAULT_SYSTEM_UPDATE_TIMEOUT: i64 = 2 * ONE_HOUR;
const DEFAULT_PREDICTION_TIMEOUT: i64 = 12 * ONE_HOUR;
const DEFAULT_AUTO_RESTRICTED_BUCKET_DELAY_MS: i64 = ONE_HOUR;

/// `SystemClock.elapsedRealtime`.
fn elapsed_realtime() -> i64 {
    super::clock_ms(libc::CLOCK_MONOTONIC_RAW)
}

/// `AppIdleHistory.AppUsageHistory`.
#[derive(Clone, Debug, PartialEq)]
struct Usage {
    last_used_elapsed: i64,
    last_used_by_user_elapsed: i64,
    last_used_screen: i64,
    last_predicted_time: i64,
    last_predicted_bucket: i32,
    current_bucket: i32,
    bucketing_reason: i32,
    /// `bucketExpiryTimesMs`, by bucket.
    expiry: BTreeMap<i32, i64>,
    last_restrict_attempt_elapsed: i64,
    last_restrict_reason: i32,
}

impl Usage {
    /// `getPackageHistory`'s new history.
    fn new() -> Usage {
        Usage {
            last_used_elapsed: i32::MIN as i64,
            last_used_by_user_elapsed: i32::MIN as i64,
            last_used_screen: i32::MIN as i64,
            last_predicted_time: i32::MIN as i64,
            last_predicted_bucket: STANDBY_BUCKET_UNKNOWN,
            current_bucket: STANDBY_BUCKET_NEVER,
            bucketing_reason: REASON_MAIN_DEFAULT,
            expiry: BTreeMap::new(),
            last_restrict_attempt_elapsed: 0,
            last_restrict_reason: 0,
        }
    }
}

/// `AppIdleHistory`: the users' histories and the time bases.
struct History {
    /// `/data/system`.
    dir: PathBuf,
    users: HashMap<i32, BTreeMap<String, Usage>>,
    elapsed_snapshot: i64,
    elapsed_duration: i64,
    screen_on_snapshot: i64,
    screen_on_duration: i64,
    screen_on: bool,
}

impl History {
    /// `AppIdleHistory(storageDir, elapsedRealtime)` and `readScreenOnTime`.
    fn new(dir: &Path, now: i64) -> History {
        let mut h = History {
            dir: dir.to_path_buf(),
            users: HashMap::new(),
            elapsed_snapshot: now,
            elapsed_duration: 0,
            screen_on_snapshot: now,
            screen_on_duration: 0,
            screen_on: false,
        };
        if let Ok(text) = std::fs::read_to_string(dir.join("screen_on_time")) {
            let mut lines = text.lines().map(|l| l.parse::<i64>());
            if let (Some(Ok(screen)), Some(Ok(elapsed))) = (lines.next(), lines.next()) {
                h.screen_on_duration = screen;
                h.elapsed_duration = elapsed;
            }
        }
        h
    }

    fn update_display(&mut self, on: bool, now: i64) {
        if on == self.screen_on {
            return;
        }
        self.screen_on = on;
        if on {
            self.screen_on_snapshot = now;
        } else {
            self.screen_on_duration += now - self.screen_on_snapshot;
            self.elapsed_duration += now - self.elapsed_snapshot;
            self.elapsed_snapshot = now;
        }
    }

    fn screen_on_time(&self, now: i64) -> i64 {
        self.screen_on_duration
            + if self.screen_on {
                now - self.screen_on_snapshot
            } else {
                0
            }
    }

    fn elapsed_time(&self, now: i64) -> i64 {
        now - self.elapsed_snapshot + self.elapsed_duration
    }

    fn user_file(&self, user: i32) -> PathBuf {
        self.dir
            .join("users")
            .join(user.to_string())
            .join("app_idle_stats.xml")
    }

    /// `getUserHistory`, read when first asked for.
    fn user(&mut self, user: i32) -> &mut BTreeMap<String, Usage> {
        if !self.users.contains_key(&user) {
            let read = match std::fs::read(self.user_file(user)) {
                Ok(bytes) => read_app_idle_times(&bytes).unwrap_or_else(|e| {
                    eprintln!(
                        "app host: app standby: unable to read app idle file for user {user}: {e}"
                    );
                    BTreeMap::new()
                }),
                Err(_) => BTreeMap::new(),
            };
            self.users.insert(user, read);
        }
        self.users.get_mut(&user).unwrap()
    }

    /// `getAppUsageHistory`: made if it has none.
    fn usage(&mut self, package: &str, user: i32) -> &mut Usage {
        self.user(user)
            .entry(package.to_string())
            .or_insert_with(Usage::new)
    }

    /// `reportUsage`.
    fn report_usage(
        &mut self,
        package: &str,
        user: i32,
        new_bucket: i32,
        usage_reason: i32,
        now: i64,
        expiry: i64,
    ) {
        let elapsed_now = self.elapsed_time(now);
        let expiry_elapsed = self.elapsed_time(expiry);
        let (screen, duration, snapshot) = (
            self.screen_on_time(now),
            self.elapsed_duration,
            self.elapsed_snapshot,
        );
        let u = self.usage(package, user);
        let mut reason = REASON_MAIN_USAGE | usage_reason;
        let user_usage = is_user_usage(reason);
        let mut new_bucket = new_bucket;
        if u.current_bucket == STANDBY_BUCKET_RESTRICTED
            && !user_usage
            && u.bucketing_reason & REASON_MAIN_MASK != REASON_MAIN_TIMEOUT
        {
            new_bucket = STANDBY_BUCKET_RESTRICTED;
            reason = u.bucketing_reason;
        } else if expiry > now {
            let current = u.expiry.get(&new_bucket).copied().unwrap_or(0);
            u.expiry.insert(new_bucket, expiry_elapsed.max(current));
            u.expiry.retain(|_, t| *t >= elapsed_now);
        }
        if now != 0 {
            u.last_used_elapsed = duration + (now - snapshot);
            if user_usage {
                u.last_used_by_user_elapsed = u.last_used_elapsed;
            }
            u.last_used_screen = screen;
        }
        if u.current_bucket >= new_bucket {
            u.current_bucket = new_bucket;
            u.bucketing_reason = reason;
        }
    }

    /// `setAppStandbyBucket`.
    fn set_bucket(&mut self, package: &str, user: i32, now: i64, bucket: i32, reason: i32) {
        let elapsed = self.elapsed_time(now);
        let u = self.usage(package, user);
        u.current_bucket = bucket;
        u.bucketing_reason = reason;
        if reason & REASON_MAIN_MASK == REASON_MAIN_PREDICTED {
            u.last_predicted_time = elapsed;
            u.last_predicted_bucket = bucket;
        }
    }

    /// `getThresholdIndex`.
    fn threshold_index(
        &mut self,
        package: &str,
        user: i32,
        now: i64,
        screen: &[i64; 5],
        elapsed: &[i64; 5],
        bypass_screen: bool,
    ) -> i32 {
        let (screen_now, elapsed_now) = (self.screen_on_time(now), self.elapsed_time(now));
        let Some(u) = self.user(user).get(package) else {
            return -1;
        };
        if u.last_used_elapsed < 0 || (!bypass_screen && u.last_used_screen < 0) {
            return -1;
        }
        let screen_delta = screen_now - u.last_used_screen;
        let elapsed_delta = elapsed_now - u.last_used_elapsed;
        for i in (0..screen.len()).rev() {
            if (bypass_screen || screen_delta >= screen[i]) && elapsed_delta >= elapsed[i] {
                return i as i32;
            }
        }
        0
    }

    /// `getAppStandbyBucket`: never used, never bucketed.
    fn bucket(&mut self, package: &str, user: i32) -> i32 {
        self.user(user)
            .get(package)
            .map_or(STANDBY_BUCKET_NEVER, |u| u.current_bucket)
    }
}

/// `AppStandbyController.isUserUsage`.
fn is_user_usage(reason: i32) -> bool {
    reason & REASON_MAIN_MASK == REASON_MAIN_USAGE
        && matches!(
            reason & REASON_SUB_MASK,
            REASON_SUB_USAGE_USER_INTERACTION | REASON_SUB_USAGE_MOVE_TO_FOREGROUND
        )
}

/// `readAppIdleTimes` of `app_idle_stats.xml`.
fn read_app_idle_times(bytes: &[u8]) -> Result<BTreeMap<String, Usage>, String> {
    let root = aim_android_xml::read(bytes)?;
    let mut users = BTreeMap::new();
    if root.name != "packages" {
        return Ok(users);
    }
    let version = root.int("version")?.unwrap_or(0);
    for p in root.children().filter(|e| e.name == "package") {
        let Some(name) = p.string("name") else {
            continue;
        };
        let long = |n: &str| p.long(n);
        let last_used_elapsed =
            long("elapsedIdleTime")?.ok_or("package without elapsedIdleTime")?;
        let mut u = Usage {
            last_used_elapsed,
            last_used_by_user_elapsed: long("lastUsedByUserElapsedTime")?
                .unwrap_or(last_used_elapsed),
            last_used_screen: long("screenIdleTime")?.ok_or("package without screenIdleTime")?,
            last_predicted_time: long("lastPredictedTime")?.unwrap_or(0),
            current_bucket: p.int("appLimitBucket")?.unwrap_or(STANDBY_BUCKET_ACTIVE),
            bucketing_reason: p
                .int_hex("bucketReason")
                .ok()
                .flatten()
                .unwrap_or(REASON_MAIN_DEFAULT),
            last_restrict_attempt_elapsed: long("lastRestrictionAttemptElapsedTime")?.unwrap_or(0),
            last_restrict_reason: p
                .int_hex("lastRestrictionAttemptReason")
                .ok()
                .flatten()
                .unwrap_or(0),
            ..Usage::new()
        };
        if version >= 1 {
            for item in p
                .children()
                .filter(|e| e.name == "expiryTimes")
                .flat_map(|e| e.children())
                .filter(|e| e.name == "item")
            {
                let bucket = item.int("bucket")?.unwrap_or(STANDBY_BUCKET_UNKNOWN);
                if bucket == STANDBY_BUCKET_UNKNOWN {
                    continue;
                }
                u.expiry.insert(bucket, item.long("expiry")?.unwrap_or(0));
            }
        } else {
            let active = long("activeTimeoutTime")?.unwrap_or(0);
            let working_set = long("workingSetTimeoutTime")?.unwrap_or(0);
            if active != 0 || working_set != 0 {
                u.expiry.insert(STANDBY_BUCKET_ACTIVE, active);
                u.expiry.insert(STANDBY_BUCKET_WORKING_SET, working_set);
            }
        }
        users.insert(name.into_owned(), u);
    }
    Ok(users)
}

/// `ConstantsObserver`'s values.
#[derive(Clone, Debug, PartialEq)]
struct Constants {
    screen_thresholds: [i64; 5],
    elapsed_thresholds: [i64; 5],
    check_idle_interval: i64,
    strong_usage_timeout: i64,
    system_update_timeout: i64,
    prediction_timeout: i64,
    auto_restricted_bucket_delay: i64,
}

impl Constants {
    /// The defaults and the `app_standby` DeviceConfig namespace's
    /// properties (`processDefaultConstants`, `processProperties`).
    fn read(property: &dyn Fn(&str) -> Option<String>, adjust_defaults: bool) -> Constants {
        let long = |key: &str, default: i64| {
            property(key)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(default)
        };
        let thresholds = |prefix: &str, defaults: [i64; 5], minimums: [i64; 5]| {
            let keys: Vec<String> = BUCKET_NAMES
                .iter()
                .map(|b| format!("{prefix}{b}"))
                .collect();
            if keys.iter().all(|k| property(k).is_none()) {
                return defaults;
            }
            std::array::from_fn(|i| minimums[i].max(long(&keys[i], defaults[i])))
        };
        let threshold_keys = BUCKET_NAMES.iter().any(|b| {
            property(&format!("screen_threshold_{b}")).is_some()
                || property(&format!("elapsed_threshold_{b}")).is_some()
        });
        let (screen_thresholds, elapsed_thresholds) = if threshold_keys {
            (
                thresholds(
                    "screen_threshold_",
                    DEFAULT_SCREEN_TIME_THRESHOLDS,
                    MINIMUM_SCREEN_TIME_THRESHOLDS,
                ),
                thresholds(
                    "elapsed_threshold_",
                    DEFAULT_ELAPSED_TIME_THRESHOLDS,
                    MINIMUM_ELAPSED_TIME_THRESHOLDS,
                ),
            )
        } else {
            (
                DEFAULT_SCREEN_TIME_THRESHOLDS,
                DEFAULT_ELAPSED_TIME_THRESHOLDS,
            )
        };
        let strong_default = if adjust_defaults {
            DEFAULT_CURRENT_STRONG_USAGE_TIMEOUT
        } else {
            DEFAULT_LEGACY_STRONG_USAGE_TIMEOUT
        };
        Constants {
            screen_thresholds,
            elapsed_thresholds,
            check_idle_interval: (elapsed_thresholds[1] / 4).min(DEFAULT_CHECK_IDLE_INTERVAL_MS),
            strong_usage_timeout: long("strong_usage_duration", strong_default),
            system_update_timeout: long(
                "system_update_usage_duration",
                DEFAULT_SYSTEM_UPDATE_TIMEOUT,
            ),
            prediction_timeout: long("prediction_timeout", DEFAULT_PREDICTION_TIMEOUT),
            auto_restricted_bucket_delay: match property("auto_restricted_bucket_delay_ms") {
                Some(_) => (4 * ONE_HOUR).max(long(
                    "auto_restricted_bucket_delay_ms",
                    DEFAULT_AUTO_RESTRICTED_BUCKET_DELAY_MS,
                )),
                None => DEFAULT_AUTO_RESTRICTED_BUCKET_DELAY_MS,
            },
        }
    }
}

/// What the controller decides with, read once its services are ready.
struct Controller {
    enabled: bool,
    constants: Constants,
    history: History,
    /// `mPowerWhitelistedApps`: DeviceIdleController's
    /// `getFullPowerWhitelistExceptIdle`.
    power_allowlist: BTreeSet<String>,
    /// `mHeadlessSystemApps`.
    headless: BTreeSet<String>,
    /// `Flags.screenTimeBypass`.
    screen_time_bypass: bool,
}

/// App standby: the controller, made when first needed.
#[derive(Default)]
pub(super) struct AppStandby {
    controller: Mutex<Option<Controller>>,
}

/// An app usage event AppStandbyController takes (`onUsageEvent`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Event {
    ActivityResumed,
    ActivityPaused,
}

impl AppHost {
    /// `AppStandbyController.Injector.isAppIdleEnabled`.
    pub(super) fn app_idle_enabled(&self) -> Result<bool, Exception> {
        let inputs = self.inputs()?;
        let global = |name: &str| {
            inputs
                .settings
                .get(Table::Global, name)
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(1)
                == 1
        };
        Ok(inputs
            .packages
            .framework_bool("config_enableAutoPowerModes")
            .unwrap_or(false)
            && global("app_standby_enabled")
            && global("adaptive_battery_management_enabled"))
    }

    /// Runs `f` on the controller, made (and the apps' buckets first
    /// checked) when first needed.
    fn with_standby<T>(
        self: &Arc<Self>,
        f: impl FnOnce(&AppHost, &mut Controller) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let mut guard = self.app_standby.controller.lock().unwrap();
        if guard.is_none() {
            let controller = self.start_standby()?;
            *guard = Some(controller);
            let me = Arc::downgrade(self);
            let interval = guard.as_ref().unwrap().constants.check_idle_interval;
            std::thread::spawn(move || check_idle_states_periodically(me, interval));
        }
        f(self, guard.as_mut().unwrap())
    }

    /// `AppStandbyController`'s start and boot: the constants, the
    /// history, the display on (`PHASE_SYSTEM_SERVICES_READY`), the
    /// system apps' defaults for a user without a history, the power
    /// allowlist and the headless system apps (`PHASE_BOOT_COMPLETED`),
    /// then every app's bucket checked (`postCheckIdleStates`).
    fn start_standby(&self) -> Result<Controller, Exception> {
        let inputs = self.inputs()?;
        let enabled = self.app_idle_enabled()?;
        let property = |key: &str| {
            inputs
                .settings
                .get(Table::Config, &format!("app_standby/{key}"))
        };
        let constants = Constants::read(
            &property,
            self.flag("com.android.server.usage.adjust_default_bucket_elevation_params")?,
        );
        let now = elapsed_realtime();
        let mut history = History::new(&inputs.data.join("system"), now);
        history.update_display(true, now);
        let mut c = Controller {
            enabled,
            constants,
            history,
            power_allowlist: self.power_allowlist()?,
            headless: self.headless_system_apps()?,
            screen_time_bypass: self.flag("com.android.server.usage.screen_time_bypass")?,
        };
        if !c.history.user_file(0).exists() {
            self.initialize_defaults_for_system_apps(&mut c, 0, now)?;
        }
        self.check_idle_states(&mut c, now)?;
        Ok(c)
    }

    /// `initializeDefaultsForSystemApps`: every system app active for the
    /// system update timeout.
    fn initialize_defaults_for_system_apps(
        &self,
        c: &mut Controller,
        user: i32,
        now: i64,
    ) -> Result<(), Exception> {
        for (package, _, flags) in self.installed(MATCH_DISABLED_COMPONENTS, user)? {
            if flags & FLAG_SYSTEM != 0 {
                c.history.report_usage(
                    &package,
                    user,
                    STANDBY_BUCKET_ACTIVE,
                    REASON_SUB_USAGE_SYSTEM_UPDATE,
                    0,
                    now + c.constants.system_update_timeout,
                );
            }
        }
        Ok(())
    }

    /// The packages installed for `user` with `flags`: name, uid and
    /// application flags.
    fn installed(&self, flags: i64, user: i32) -> Result<Vec<(String, i32, i32)>, Exception> {
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let packages = query.installed_packages(flags, user).map_err(|e| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("app standby: installed packages: {}", e.0),
            )
        })??;
        Ok(packages
            .into_iter()
            .filter_map(|p| {
                let info = p.application_info?;
                Some((p.package_name?, info.uid, info.flags))
            })
            .collect())
    }

    /// DeviceIdleController's `getFullPowerWhitelistExceptIdle`: the
    /// installed system apps SystemConfig allows in power save (except
    /// idle, or at all), and the user's (`deviceidle.xml`'s `wl`).
    fn power_allowlist(&self) -> Result<BTreeSet<String>, Exception> {
        let (state, _) = self.packages()?;
        let system: BTreeSet<String> = self
            .installed(MATCH_ANY_USER | MATCH_SYSTEM_ONLY, 0)?
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        let mut apps: BTreeSet<String> = state
            .system
            .allow_in_power_save_except_idle
            .iter()
            .chain(&state.system.allow_in_power_save)
            .filter(|p| system.contains(*p))
            .cloned()
            .collect();
        let installed: BTreeSet<String> = self
            .installed(MATCH_ANY_USER, 0)?
            .into_iter()
            .map(|(n, _, _)| n)
            .collect();
        if let Ok(bytes) = std::fs::read(self.inputs()?.data.join("system/deviceidle.xml")) {
            let root = aim_android_xml::read(&bytes)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("deviceidle.xml: {e}")))?;
            apps.extend(
                root.children()
                    .filter(|e| e.name == "wl")
                    .filter_map(|e| e.string("n"))
                    .map(|n| n.into_owned())
                    .filter(|n| installed.contains(n)),
            );
        }
        Ok(apps)
    }

    /// `loadHeadlessSystemAppCache`: the system apps without a launcher
    /// activity.
    fn headless_system_apps(&self) -> Result<BTreeSet<String>, Exception> {
        let flags = MATCH_DIRECT_BOOT_AWARE_UNAWARE | MATCH_DISABLED_COMPONENTS | MATCH_SYSTEM_ONLY;
        let (_, resolution) = self.packages()?;
        let intent = crate::package::intent::Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec!["android.intent.category.LAUNCHER".into()]),
            ..Default::default()
        };
        let launchers: BTreeSet<String> = resolution
            .query_intent_activities(&intent, None, flags, 0, SYSTEM_UID)
            .map_err(|e| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("app standby: launcher activities: {e:?}"),
                )
            })?
            .into_iter()
            .filter_map(|r| match r.info {
                crate::package::component_resolver::Info::Activity(a) => {
                    a.info.application_info.item.package_name.clone()
                }
                _ => None,
            })
            .collect();
        Ok(self
            .installed(flags, 0)?
            .into_iter()
            .map(|(n, _, _)| n)
            .filter(|n| !launchers.contains(n))
            .collect())
    }

    /// `checkIdleStates` for the running user.
    fn check_idle_states(&self, c: &mut Controller, now: i64) -> Result<(), Exception> {
        if !c.enabled {
            return Ok(());
        }
        for (package, uid, _) in self.installed(MATCH_DISABLED_COMPONENTS, 0)? {
            self.check_and_update(c, &package, 0, uid, now)?;
        }
        Ok(())
    }

    /// `checkAndUpdateStandbyState`.
    fn check_and_update(
        &self,
        c: &mut Controller,
        package: &str,
        user: i32,
        uid: i32,
        now: i64,
    ) -> Result<(), Exception> {
        let min_bucket = self.app_min_bucket(c, package, uid % PER_USER_RANGE as i32, user)?;
        if min_bucket <= STANDBY_BUCKET_ACTIVE {
            c.history
                .set_bucket(package, user, now, min_bucket, REASON_MAIN_DEFAULT);
            return Ok(());
        }
        let elapsed = c.history.elapsed_time(now);
        let app = c.history.usage(package, user).clone();
        let mut reason = app.bucketing_reason;
        let old_main_reason = reason & REASON_MAIN_MASK;
        if old_main_reason == REASON_MAIN_FORCED_BY_USER
            || app.current_bucket == STANDBY_BUCKET_NEVER
        {
            return Ok(());
        }
        let mut new_bucket = app.current_bucket.max(STANDBY_BUCKET_ACTIVE);
        let prediction_late = app.last_predicted_time > 0
            && elapsed - app.last_predicted_time > c.constants.prediction_timeout;
        if matches!(
            old_main_reason,
            REASON_MAIN_DEFAULT | REASON_MAIN_USAGE | REASON_MAIN_TIMEOUT
        ) || prediction_late
        {
            if !prediction_late
                && (STANDBY_BUCKET_ACTIVE..=STANDBY_BUCKET_RARE)
                    .contains(&app.last_predicted_bucket)
            {
                new_bucket = app.last_predicted_bucket;
                reason = REASON_MAIN_PREDICTED | REASON_SUB_PREDICTED_RESTORED;
            } else if !(old_main_reason == REASON_MAIN_DEFAULT
                && app.bucketing_reason & REASON_SUB_MASK == REASON_SUB_DEFAULT_APP_RESTORED)
            {
                let (screen, elapsed_thresholds) = (
                    c.constants.screen_thresholds,
                    c.constants.elapsed_thresholds,
                );
                let index = c.history.threshold_index(
                    package,
                    user,
                    now,
                    &screen,
                    &elapsed_thresholds,
                    c.screen_time_bypass,
                );
                new_bucket = if index >= 0 {
                    THRESHOLD_BUCKETS[index as usize]
                } else {
                    STANDBY_BUCKET_NEVER
                };
                reason = REASON_MAIN_TIMEOUT;
            }
        }
        // `getMinBucketWithValidExpiryTime`.
        if let Some((&bucket, _)) = app
            .expiry
            .iter()
            .take_while(|(b, _)| new_bucket > **b)
            .find(|(_, t)| **t > elapsed)
        {
            new_bucket = bucket;
            reason = if new_bucket == STANDBY_BUCKET_ACTIVE || app.current_bucket == new_bucket {
                app.bucketing_reason
            } else {
                REASON_MAIN_USAGE | REASON_SUB_USAGE_ACTIVE_TIMEOUT
            };
        }
        if app.last_used_by_user_elapsed >= 0
            && app.last_restrict_attempt_elapsed > app.last_used_by_user_elapsed
            && elapsed - app.last_used_by_user_elapsed >= c.constants.auto_restricted_bucket_delay
        {
            new_bucket = STANDBY_BUCKET_RESTRICTED;
            reason = app.last_restrict_reason;
        }
        new_bucket = new_bucket.min(min_bucket);
        if app.current_bucket != new_bucket || prediction_late {
            c.history.set_bucket(package, user, now, new_bucket, reason);
        }
        Ok(())
    }

    /// `getAppMinBucket`: the lowest bucket `package` should ever enter.
    fn app_min_bucket(
        &self,
        c: &Controller,
        package: &str,
        app_id: i32,
        user: i32,
    ) -> Result<i32, Exception> {
        if !c.enabled
            || app_id < FIRST_APPLICATION_UID
            || package == "android"
            || c.power_allowlist.contains(package)
        {
            return Ok(STANDBY_BUCKET_EXEMPTED);
        }
        // `isActiveDeviceAdmin`, `isAdminProtectedPackages` and
        // `isActiveNetworkScorer` need what DevicePolicyManagerService and
        // NetworkScoreService would tell.
        let policies = self.device_policies()?;
        if policies.has_admins_or_policies() {
            return Err(self.unimplemented_named(
                "usagestats",
                "app standby with device admins or policies (#1723)",
            ));
        }
        let inputs = self.inputs()?;
        if inputs
            .settings
            .get(Table::Global, "network_recommendations_package")
            .as_deref()
            == Some(package)
            && inputs
                .settings
                .get(Table::Global, "network_recommendations_enabled")
                .as_deref()
                != Some("-1")
        {
            return Err(self.unimplemented_named(
                "usagestats",
                "app standby of the network recommendations package (#1723)",
            ));
        }
        let uid = user * PER_USER_RANGE as i32 + app_id;
        if super::appops_service::mode(
            self,
            super::appops_service::OP_SYSTEM_EXEMPT_FROM_POWER_RESTRICTIONS,
            uid,
            Some(package),
        )? == super::appops_service::MODE_ALLOWED
        {
            return Ok(STANDBY_BUCKET_EXEMPTED);
        }
        if self.bound_widget_packages(user)?.contains(package) {
            return Ok(STANDBY_BUCKET_ACTIVE);
        }
        let config = |name: &str| inputs.packages.framework_string(name).unwrap_or_default();
        if config("config_deviceProvisioningPackage") == package {
            return Ok(STANDBY_BUCKET_EXEMPTED);
        }
        if config("config_systemWellbeing") == package
            || self.exact_alarm_bucket_elevation(package, uid)?
        {
            return Ok(STANDBY_BUCKET_WORKING_SET);
        }
        // Carrier apps: the app host has no subscriptions, so none has
        // carrier privileges.
        if c.headless.contains(package) {
            return Ok(STANDBY_BUCKET_ACTIVE);
        }
        if self.check_package_permission(ACCESS_BACKGROUND_LOCATION, package, 0)? {
            return Ok(STANDBY_BUCKET_FREQUENT);
        }
        Ok(STANDBY_BUCKET_NEVER)
    }

    /// AppWidgetService's `isBoundWidgetPackage` for `user`: the providers
    /// of the widgets its persisted state (`appwidgets.xml`) holds.
    fn bound_widget_packages(&self, user: i32) -> Result<BTreeSet<String>, Exception> {
        let path = self
            .inputs()?
            .data
            .join(format!("system/users/{user}/appwidgets.xml"));
        let Ok(bytes) = std::fs::read(&path) else {
            return Ok(BTreeSet::new());
        };
        let root = aim_android_xml::read(&bytes)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("appwidgets.xml: {e}")))?;
        if root.children().any(|e| e.name == "g") {
            return Err(
                self.unimplemented_named("usagestats", "app standby with bound widgets (#1683)")
            );
        }
        Ok(BTreeSet::new())
    }

    /// AlarmManagerService's `shouldGetBucketElevation`:
    /// `hasUseExactAlarmInternal`, or `hasScheduleExactAlarmInternal`
    /// where scheduling exact alarms elevates the bucket.
    fn exact_alarm_bucket_elevation(&self, package: &str, uid: i32) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
        let Some(ps) = state.packages.get(package) else {
            return Ok(false);
        };
        let compat = &self.inputs()?.compat;
        let target = ps.target_sdk_version;
        let enabled = |change| compat.is_enabled(change, Some(package), target);
        if enabled(ENABLE_USE_EXACT_ALARM)
            && super::permission_checker::preflight(self, USE_EXACT_ALARM, uid, package)?
        {
            return Ok(true);
        }
        if enabled(SCHEDULE_EXACT_ALARM_DOES_NOT_ELEVATE_BUCKET) {
            return Ok(false);
        }
        // `mExactAlarmCandidates`: the apps that ask for the permission.
        let candidate = ps.pkg.as_ref().is_some_and(|p| {
            p.requested_permissions
                .iter()
                .any(|r| r == SCHEDULE_EXACT_ALARM)
        });
        if !candidate || !enabled(REQUIRE_EXACT_ALARM_PERMISSION) {
            return Ok(false);
        }
        if enabled(SCHEDULE_EXACT_ALARM_DENIED_BY_DEFAULT) {
            return super::permission_checker::preflight(self, SCHEDULE_EXACT_ALARM, uid, package);
        }
        let mode = super::appops_service::mode(
            self,
            super::appops_service::OP_SCHEDULE_EXACT_ALARM,
            uid,
            Some(package),
        )?;
        Ok(mode == super::appops_service::MODE_DEFAULT
            || mode == super::appops_service::MODE_ALLOWED)
    }

    /// `PackageManager.checkPermission(permission, package)` for `user`.
    fn check_package_permission(
        &self,
        permission: &str,
        package: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        let (state, resolution) = self.packages()?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let uid = query
            .package_uid(package, 0, user)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("app standby: {}", e.0)))??;
        Ok(uid >= 0 && self.check_permission(Some(permission), uid)? == 0)
    }

    /// `AppStandbyController.getAppStandbyBucket`.
    pub(super) fn app_standby_bucket(
        self: &Arc<Self>,
        package: &str,
        user: i32,
    ) -> Result<i32, Exception> {
        if !self.app_idle_enabled()? {
            return Ok(STANDBY_BUCKET_EXEMPTED);
        }
        self.with_standby(|_, c| Ok(c.history.bucket(package, user)))
    }

    /// `isAppIdleFiltered(package, getAppId(package), user, now)`: idle
    /// while not charging when its bucket is rare or below and its
    /// minimum bucket lets it be.
    pub(super) fn app_idle_filtered(
        self: &Arc<Self>,
        package: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        let charging = self
            .battery_stats
            .get_or_init(|| Arc::new(super::battery_stats::BatteryStats::new(self)))
            .charging();
        if !self.app_idle_enabled()? || charging {
            return Ok(false);
        }
        // `getAppId`: of any user's install, disabled or not.
        let (state, _) = self.packages()?;
        let app_id = state.packages.get(package).map_or(-1, |ps| ps.app_id);
        self.with_standby(|host, c| {
            if c.history.bucket(package, user) < IDLE_BUCKET_CUTOFF {
                return Ok(false);
            }
            Ok(host.app_min_bucket(c, package, app_id, user)? >= IDLE_BUCKET_CUTOFF)
        })
    }

    /// `onUsageEvent` of `package`'s `event` for `user`: an activity
    /// resumed or paused holds it active for the strong usage timeout,
    /// after which its bucket is checked again.
    pub(super) fn report_usage_event(self: &Arc<Self>, package: &str, user: i32, event: Event) {
        let delay = match self.app_idle_enabled() {
            Ok(true) => self.with_standby(|_, c| {
                let now = elapsed_realtime();
                let before = c.history.usage(package, user).current_bucket;
                let sub = match event {
                    Event::ActivityResumed => REASON_SUB_USAGE_MOVE_TO_FOREGROUND,
                    Event::ActivityPaused => REASON_SUB_USAGE_MOVE_TO_BACKGROUND,
                };
                let timeout = c.constants.strong_usage_timeout;
                c.history.report_usage(
                    package,
                    user,
                    STANDBY_BUCKET_ACTIVE,
                    sub,
                    now,
                    now + timeout,
                );
                Ok((c.history.usage(package, user).current_bucket != before).then_some(timeout))
            }),
            Ok(false) => Ok(None),
            Err(e) => Err(e),
        };
        match delay {
            Ok(Some(delay)) => {
                let (me, package) = (Arc::downgrade(self), package.to_string());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(delay as u64));
                    if let Some(host) = me.upgrade() {
                        host.check_package_idle_state(&package, user);
                    }
                });
            }
            Ok(None) => {}
            Err(e) => eprintln!("app host: app standby: {package}: {}", e.message),
        }
    }

    /// `MSG_CHECK_PACKAGE_IDLE_STATE`.
    fn check_package_idle_state(self: &Arc<Self>, package: &str, user: i32) {
        let checked = self.with_standby(|host, c| {
            let (state, resolution) = host.packages()?;
            let query = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: SYSTEM_UID,
            };
            let uid = query
                .package_uid(package, 0, user)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.0))??;
            if uid > 0 {
                host.check_and_update(c, package, user, uid, elapsed_realtime())?;
            }
            Ok(())
        });
        if let Err(e) = checked {
            eprintln!("app host: app standby: {package}: {}", e.message);
        }
    }
}

/// `MSG_CHECK_IDLE_STATES` for the running user, every `interval`.
fn check_idle_states_periodically(host: Weak<AppHost>, interval: i64) {
    loop {
        std::thread::sleep(Duration::from_millis(interval as u64));
        let Some(host) = host.upgrade() else { return };
        let checked = host.with_standby(|h, c| h.check_idle_states(c, elapsed_realtime()));
        if let Err(e) = checked {
            eprintln!("app host: app standby: {}", e.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATS: &str = r#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<packages version="1">
    <package name="com.example.used" elapsedIdleTime="1000" lastUsedByUserElapsedTime="1000" screenIdleTime="500" lastPredictedTime="-2147483648" appLimitBucket="20" bucketReason="307" lastRestrictionAttemptReason="0">
        <expiryTimes>
            <item bucket="10" expiry="7203882" />
        </expiryTimes>
    </package>
    <package name="com.example.never" elapsedIdleTime="-2147483648" screenIdleTime="-2147483648" appLimitBucket="5" bucketReason="100" />
</packages>"#;

    #[test]
    fn app_idle_times_read_as_the_original_reads_them() {
        let users = read_app_idle_times(STATS.as_bytes()).unwrap();
        let used = &users["com.example.used"];
        assert_eq!(
            (
                used.last_used_elapsed,
                used.last_used_by_user_elapsed,
                used.last_used_screen
            ),
            (1000, 1000, 500)
        );
        assert_eq!((used.current_bucket, used.bucketing_reason), (20, 0x307));
        assert_eq!(used.expiry, BTreeMap::from([(10, 7_203_882)]));
        let never = &users["com.example.never"];
        assert_eq!(
            (
                never.last_used_by_user_elapsed,
                never.current_bucket,
                never.bucketing_reason
            ),
            (i32::MIN as i64, 5, 0x100)
        );
    }

    fn history(at: i64) -> History {
        let mut h = History::new(Path::new("/nonexistent"), at);
        h.users
            .insert(0, read_app_idle_times(STATS.as_bytes()).unwrap());
        h.update_display(true, at);
        h
    }

    #[test]
    fn usage_makes_an_app_active_until_its_expiry() {
        let mut h = history(10_000);
        h.report_usage(
            "com.example.used",
            0,
            STANDBY_BUCKET_ACTIVE,
            REASON_SUB_USAGE_MOVE_TO_FOREGROUND,
            20_000,
            20_000 + ONE_HOUR,
        );
        let u = &h.users[&0]["com.example.used"];
        assert_eq!(
            (u.current_bucket, u.bucketing_reason),
            (
                STANDBY_BUCKET_ACTIVE,
                REASON_MAIN_USAGE | REASON_SUB_USAGE_MOVE_TO_FOREGROUND
            )
        );
        // Elapsed time continues from the persisted duration (none here).
        assert_eq!(
            (
                u.last_used_elapsed,
                u.last_used_by_user_elapsed,
                u.last_used_screen
            ),
            (10_000, 10_000, 10_000)
        );
        assert_eq!(
            u.expiry[&STANDBY_BUCKET_ACTIVE],
            7_203_882.max(10_000 + ONE_HOUR)
        );
        // A worse bucket's usage does not lower a better one.
        h.report_usage(
            "com.example.used",
            0,
            STANDBY_BUCKET_WORKING_SET,
            REASON_SUB_USAGE_MOVE_TO_BACKGROUND,
            30_000,
            0,
        );
        assert_eq!(
            h.users[&0]["com.example.used"].current_bucket,
            STANDBY_BUCKET_ACTIVE
        );
        assert_eq!(h.bucket("com.example.absent", 0), STANDBY_BUCKET_NEVER);
    }

    #[test]
    fn thresholds_index_the_time_since_use() {
        let mut h = history(0);
        let (screen, elapsed) = (
            DEFAULT_SCREEN_TIME_THRESHOLDS,
            DEFAULT_ELAPSED_TIME_THRESHOLDS,
        );
        // Used at elapsed 1000, screen 500: 13 hours later, with the screen
        // on all along, it is in the working set.
        assert_eq!(
            h.threshold_index(
                "com.example.used",
                0,
                13 * ONE_HOUR,
                &screen,
                &elapsed,
                false
            ),
            1
        );
        assert_eq!(
            h.threshold_index("com.example.used", 0, 9 * ONE_DAY, &screen, &elapsed, false),
            4
        );
        assert_eq!(
            h.threshold_index("com.example.never", 0, ONE_DAY, &screen, &elapsed, false),
            -1
        );
        assert_eq!(
            h.threshold_index("com.example.absent", 0, ONE_DAY, &screen, &elapsed, false),
            -1
        );
    }

    #[test]
    fn constants_come_from_device_config_within_their_minimums() {
        let none = Constants::read(&|_| None, true);
        assert_eq!(
            (none.strong_usage_timeout, none.check_idle_interval),
            (5 * ONE_MINUTE, 3 * ONE_HOUR)
        );
        assert_eq!(
            Constants::read(&|_| None, false).strong_usage_timeout,
            ONE_HOUR
        );
        let set = Constants::read(
            &|k| (k == "elapsed_threshold_working_set").then(|| "60000".into()),
            true,
        );
        assert_eq!(
            set.elapsed_thresholds,
            [0, ONE_HOUR, 24 * ONE_HOUR, 48 * ONE_HOUR, 8 * ONE_DAY]
        );
        assert_eq!(set.screen_thresholds, DEFAULT_SCREEN_TIME_THRESHOLDS);
        assert_eq!(set.check_idle_interval, ONE_HOUR / 4);
    }
}

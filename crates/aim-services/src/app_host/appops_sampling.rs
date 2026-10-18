//! AppOpsService's runtime app op message sampling (`android-16.0.0_r1`):
//! the package and op it watches, and the message it keeps
//! (`reportRuntimeAppOpAccessMessageAndGetConfig`). The app host's start is
//! the boot: the first reports sample at boot time, and five minutes
//! later the package and op are resampled among the packages that request
//! a dangerous permission. The rarely used packages are not listed: the
//! original takes them from a week of access history, which the app host
//! does not keep (#1593). The sampled message is not kept: only the
//! system server and instrumentation collect it
//! (`collectRuntimeAppOpAccessMessage`), and neither runs here.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::app_op_infos::{NUM_OP, OP_NONE};

/// `AppOpsManager.SAMPLING_STRATEGY_*`.
const SAMPLING_STRATEGY_UNIFORM: i32 = 1;
const SAMPLING_STRATEGY_BOOT_TIME_SAMPLING: i32 = 3;
const SAMPLING_STRATEGY_UNIFORM_OPS: i32 = 4;
/// `RARELY_USED_PACKAGES_INITIALIZATION_DELAY_MILLIS`.
const RESAMPLE_DELAY: Duration = Duration::from_secs(300);

/// `MessageSamplingConfig`.
#[derive(Debug, PartialEq)]
pub struct Config {
    sampled_op: i32,
    acceptable_left_distance: i32,
    expiration_millis: i64,
}

impl Config {
    /// What the service answers: valid for an hour from now, by the wall
    /// clock (`Instant.now().plus(1, HOURS).toEpochMilli()`).
    fn new(sampled_op: i32, acceptable_left_distance: i32) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        Config { sampled_op, acceptable_left_distance, expiration_millis: now + 3_600_000 }
    }
}

impl WriteParcelable for Config {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.sampled_op);
        p.write_i32(self.acceptable_left_distance);
        p.write_i64(self.expiration_millis);
    }
}

/// A `SyncNotedAppOp` as reported: its op.
pub struct Noted {
    op: i32,
}

impl ReadParcelable for Noted {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let fields = r.read_i32()?;
        let _mode = r.read_i32()?;
        let op = r.read_i32()?;
        for field in [0x4, 0x8] {
            if fields & field != 0 {
                r.read_string16()?;
            }
        }
        Ok(Noted { op })
    }
}

/// A uniform random integer below `bound` (`ThreadLocalRandom.nextInt`).
fn random_below(bound: u32) -> u32 {
    // SAFETY: arc4random_uniform takes and returns plain integers.
    unsafe { libc::arc4random_uniform(bound) }
}

/// A uniform random float in [0, 1) (`ThreadLocalRandom.nextFloat`).
fn random_float() -> f32 {
    random_below(1 << 24) as f32 / (1 << 24) as f32
}

/// `AppOpsManager.leftCircularDistance`.
fn left_circular_distance(from: i32, to: i32, size: i32) -> i32 {
    (to + size - from) % size
}

pub struct Sampling {
    started: Instant,
    resampled: bool,
    package: Option<String>,
    op: i32,
    acceptable_left_distance: i32,
    strategy: i32,
}

impl Sampling {
    pub fn new() -> Self {
        Sampling {
            started: Instant::now(),
            resampled: false,
            package: None,
            op: OP_NONE,
            acceptable_left_distance: 0,
            strategy: 0,
        }
    }

    /// `reportRuntimeAppOpAccessMessageAndGetConfig` of `package`'s `noted`
    /// op; `targets` are the packages sampled from
    /// (`getPackageNamesForSampling`), asked for once the delay passed.
    pub fn report(
        &mut self,
        package: &str,
        noted: Option<&Noted>,
        message: Option<&str>,
        targets: impl FnOnce() -> Vec<String>,
    ) -> Result<Config, &'static str> {
        // The handler's delayed `getPackageListAndResample`, run before the
        // report it would have preceded.
        if !self.resampled && self.started.elapsed() >= RESAMPLE_DELAY {
            self.resampled = true;
            self.resample_package_and_op(&targets());
        }
        self.switch_package_if_boot_time(package);
        if self.package.as_deref() != Some(package) {
            return Ok(Config::new(OP_NONE, 0));
        }
        let noted = noted.ok_or("notedAppOp")?;
        message.ok_or("message")?;
        self.report_internal(noted.op);
        Ok(Config::new(self.op, self.acceptable_left_distance))
    }

    /// `switchPackageIfBootTimeOrRarelyUsedLocked`, with no rarely used
    /// packages.
    fn switch_package_if_boot_time(&mut self, package: &str) {
        if self.package.is_none() && random_float() < 0.5 {
            self.strategy = SAMPLING_STRATEGY_BOOT_TIME_SAMPLING;
            self.resample_op_for_package(package.to_string(), true);
        }
    }

    /// `resamplePackageAndAppOpLocked`.
    fn resample_package_and_op(&mut self, packages: &[String]) {
        if packages.is_empty() {
            return;
        }
        let pick_op = random_float() < 0.5;
        self.strategy = if pick_op { SAMPLING_STRATEGY_UNIFORM } else { SAMPLING_STRATEGY_UNIFORM_OPS };
        let package = packages[random_below(packages.len() as u32) as usize].clone();
        self.resample_op_for_package(package, pick_op);
    }

    /// `resampleAppOpForPackageLocked`.
    fn resample_op_for_package(&mut self, package: String, pick_op: bool) {
        self.op = if pick_op { random_below(NUM_OP as u32) as i32 } else { OP_NONE };
        self.acceptable_left_distance = NUM_OP - 1;
        self.package = Some(package);
    }

    /// `reportRuntimeAppOpAccessMessageInternalLocked`: the closest op
    /// seen narrows what is watched.
    fn report_internal(&mut self, op: i32) {
        let distance = left_circular_distance(op, self.op, NUM_OP);
        if self.strategy != SAMPLING_STRATEGY_UNIFORM_OPS && distance < self.acceptable_left_distance {
            self.acceptable_left_distance = distance;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Another package than the sampled one gets the config of no op; the
    /// sampled one's closest op narrows the distance it gets back.
    #[test]
    fn reports_narrow_the_sampled_package_and_op() {
        let mut s = Sampling::new();
        s.resample_op_for_package("a".into(), true);
        s.strategy = SAMPLING_STRATEGY_BOOT_TIME_SAMPLING;
        let other = s.report("b", None, None, Vec::new).unwrap();
        assert_eq!((other.sampled_op, other.acceptable_left_distance), (OP_NONE, 0));
        let op = (s.op + 3) % NUM_OP;
        let config = s.report("a", Some(&Noted { op }), Some("trace"), Vec::new).unwrap();
        assert_eq!((config.sampled_op, config.acceptable_left_distance), (s.op, NUM_OP - 3));
        assert_eq!(s.report("a", None, Some("trace"), Vec::new), Err("notedAppOp"));
    }

    #[test]
    fn left_circular_distance_wraps() {
        assert_eq!(left_circular_distance(5, 2, NUM_OP), NUM_OP - 3);
        assert_eq!(left_circular_distance(2, 5, NUM_OP), 3);
    }
}

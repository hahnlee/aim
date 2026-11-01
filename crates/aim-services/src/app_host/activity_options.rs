//! Launch geometry from ActivityOptions / SafeActivityOptions, android-16.0.0_r1.
use super::{AppHost, config::Rect, kept::RawBundle};
use aim_binder_host::parcel::Exception;

#[derive(Clone, Copy, Default)]
pub(super) struct LaunchOptions {
    pub mode: i32,
    pub bounds: Option<Rect>,
}

impl LaunchOptions {
    pub(super) fn windowing_mode(self, windows: bool) -> i32 {
        match self.mode {
            0 => {
                if windows {
                    5
                } else {
                    1
                }
            }
            5 if windows => 5,
            // TaskDisplayArea.validateWindowingMode: an unsupported mode inherits
            // the fullscreen display area. There is no native PiP/split root.
            _ => 1,
        }
    }
}

impl AppHost {
    pub(super) fn activity_launch_options(
        &self,
        options: Option<&RawBundle>,
        uid: i32,
    ) -> Result<LaunchOptions, Exception> {
        let Some(o) = options else {
            return Ok(LaunchOptions::default());
        };
        let bad = |_| Exception::illegal_argument("ActivityOptions bundle");
        let display = o.int("android.activity.launchDisplayId", -1).map_err(bad)?;
        // The native DisplayManager publishes only display 0; no overlay displays
        // are registered with this owner yet. SafeActivityOptions checks access
        // before launch-param fallback, including for a nonexistent display.
        if display != -1 && display != 0 {
            return Err(Exception::security(format!(
                "Permission Denial: starting activity (uid={uid}) with launchDisplayId={display}"
            )));
        }
        if o.boolean("android:activity.lockTaskMode", false)
            .map_err(bad)?
        {
            // No native package is allowlisted for lock task mode.
            return Err(Exception::security(
                "Permission Denial: starting activity with lockTaskMode=true",
            ));
        }
        for (unsupported, name) in [
            (
                o.int("android.activity.launchTaskId", -1).map_err(bad)? != -1,
                "task targeting",
            ),
            (
                o.int("android.activity.launchTaskDisplayAreaFeatureId", -1)
                    .map_err(bad)?
                    != -1,
                "task display area selection",
            ),
            (
                !matches!(
                    o.int("android.activity.activityType", 0).map_err(bad)?,
                    0 | 1
                ),
                "activity type",
            ),
            (
                o.boolean("android.activity.taskOverlay", false)
                    .map_err(bad)?,
                "task overlay",
            ),
            (
                o.int("android.activity.splashScreenStyle", -1)
                    .map_err(bad)?
                    != -1,
                "splash screen style",
            ),
            (
                o.boolean("android.activity.dismissKeyguardIfInsecure", false)
                    .map_err(bad)?,
                "dismiss keyguard",
            ),
        ] {
            if unsupported {
                return Err(self.unimplemented_named(
                    "activity_task",
                    &format!("{name} launch option (#1485)"),
                ));
            }
        }
        Ok(LaunchOptions {
            mode: o.int("android.activity.windowingMode", 0).map_err(bad)?,
            bounds: o
                .rect("android:activity.launchBounds")
                .map_err(bad)?
                .filter(|b| b.width() > 0 && b.height() > 0),
        })
    }
}

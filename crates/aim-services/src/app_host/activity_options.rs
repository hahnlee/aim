//! Launch geometry from ActivityOptions / SafeActivityOptions, android-16.0.0_r1.
use super::{AppHost, config::Rect, kept::RawBundle};
use aim_binder_host::parcel::Exception;

#[derive(Clone, Copy, Default)]
pub(super) struct LaunchOptions {
    /// The requested windowing mode (`getLaunchWindowingMode`), unvalidated.
    pub mode: i32,
    pub bounds: Option<Rect>,
    /// The activity type of the start's root task (`resolveActivityType`):
    /// home of a home activity's (`ActivityRecord.setActivityType`) or
    /// `startHomeActivity`'s, else unset (standard).
    pub activity_type: i32,
    /// `getAvoidMoveToFront`: the start brings no task to the front.
    pub avoid_move_to_front: bool,
    /// `getLaunchRootTask`: the organizer root task the start goes into,
    /// none if its token names no task (`Task.fromWindowContainerToken`).
    pub launch_root: Option<i32>,
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
        let launch_root = o
            .window_container_token("android.activity.launchRootTaskToken")
            .map_err(bad)?
            .and_then(|token| self.task_of_token(token))
            .filter(|&id| self.tasks.bounds(id).is_some());
        // Into an app task, a task nested in another (#2616).
        if launch_root.is_some_and(|id| self.tasks.root(id).is_none()) {
            return Err(self.unimplemented_named(
                "activity_task",
                "a launch root task not created by an organizer (#2616)",
            ));
        }
        Ok(LaunchOptions {
            mode: o.int("android.activity.windowingMode", 0).map_err(bad)?,
            bounds: o
                .rect("android:activity.launchBounds")
                .map_err(bad)?
                .filter(|b| b.width() > 0 && b.height() > 0),
            avoid_move_to_front: o
                .boolean("android.activity.avoidMoveToFront", false)
                .map_err(bad)?,
            launch_root,
            ..Default::default()
        })
    }
}

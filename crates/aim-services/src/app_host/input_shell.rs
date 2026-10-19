//! `cmd input` (`input` in the guest's shell): InputShellCommand at
//! `android-16.0.0_r1` for the touchscreen: `tap` and `swipe`, injected as
//! `injectMotionEvent` makes them (`InputManagerService.injectInputEvent`,
//! with INJECT_EVENTS) through the app host agent's dispatcher, which
//! delivers them as it delivers the touchscreen's. Other sources and
//! commands, and displays but the default one, are not implemented
//! (#1678). The injection does not wait for the window to finish the
//! event.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::time::Duration;

use aim_binder_host::parcel::Exception;

use super::AppHost;
use crate::shell::ShellCommand;

const INJECT_EVENTS: &str = "android.permission.INJECT_EVENTS";
/// The agent's `injectMotion(int action, long downTimeNanos, long
/// eventTimeNanos, float x, float y, float pressure)`.
const INJECT_MOTION: u32 = 7;
/// `MotionEvent.ACTION_DOWN`, `ACTION_UP`, `ACTION_MOVE`.
const ACTION_DOWN: i32 = 0;
const ACTION_UP: i32 = 1;
const ACTION_MOVE: i32 = 2;
/// `InputShellCommand`'s `SOURCES`.
const SOURCES: &[&str] = &[
    "keyboard",
    "dpad",
    "gamepad",
    "touchscreen",
    "mouse",
    "stylus",
    "trackball",
    "touchpad",
    "touchnavigation",
    "joystick",
    "rotaryencoder",
];
const INVALID_ARGUMENTS: &str = "Error: Invalid arguments for command: ";
/// `SWIPE_EVENT_HZ_DEFAULT`, and a swipe's default duration.
const SWIPE_EVENT_HZ: f32 = 120.0;
const SWIPE_DURATION_MS: i64 = 300;

/// `SystemClock.uptimeMillis`.
fn uptime_ms() -> i64 {
    aim_hostcall::clock::monotonic_ns() / 1_000_000
}

/// `InputShellCommand.onCommand` for `uid`, the shell command's result, as
/// `ShellCommand.exec` gives it.
pub fn run(host: &AppHost, uid: i32, c: &mut ShellCommand) -> i32 {
    match on_command(host, uid, c) {
        Ok(()) => 0,
        Err(e) => {
            c.exception(&e);
            -1
        }
    }
}

fn on_command(host: &AppHost, uid: i32, c: &mut ShellCommand) -> Result<(), String> {
    let mut arg = c.command().unwrap_or_default().to_string();
    if SOURCES.contains(&arg.as_str()) {
        if arg != "touchscreen" {
            return Err(unimplemented(host, &format!("the {arg} source")));
        }
        arg = c.next_arg_required()?;
    }
    if arg == "-d" {
        let display = c.next_arg_required()?;
        if !matches!(
            display.as_str(),
            "0" | "DEFAULT_DISPLAY" | "INVALID_DISPLAY" | "-1"
        ) {
            return Err(unimplemented(host, &format!("display {display}")));
        }
        arg = c.next_arg_required()?;
    }
    let float = |c: &mut ShellCommand, arg: &str| -> Result<f32, String> {
        let v = c.next_arg_required()?;
        v.trim()
            .parse::<f32>()
            .map_err(|_| format!("java.lang.IllegalArgumentException: {INVALID_ARGUMENTS}{arg}"))
    };
    match arg.as_str() {
        // `sendTap`.
        "tap" => {
            let (x, y) = (float(c, &arg)?, float(c, &arg)?);
            let now = uptime_ms();
            inject(host, uid, ACTION_DOWN, now, now, x, y, 1.0)?;
            inject(host, uid, ACTION_UP, now, now, x, y, 0.0)
        }
        // `sendSwipe`: moves at most at SWIPE_EVENT_HZ, interpolated.
        "swipe" => {
            let (x1, y1, x2, y2) = (
                float(c, &arg)?,
                float(c, &arg)?,
                float(c, &arg)?,
                float(c, &arg)?,
            );
            let duration = match c.next_arg() {
                Some(d) => d.trim().parse::<i32>().map_err(|_| {
                    format!("java.lang.IllegalArgumentException: {INVALID_ARGUMENTS}{arg}")
                })? as i64,
                None => -1,
            };
            let duration = if duration < 0 {
                SWIPE_DURATION_MS
            } else {
                duration
            };
            let down = uptime_ms();
            inject(host, uid, ACTION_DOWN, down, down, x1, y1, 1.0)?;
            let mut now = uptime_ms();
            let end = down + duration;
            let period = 1000.0 / SWIPE_EVENT_HZ;
            let mut injected = 1;
            while now < end {
                let error = (injected as f32 * period - (now - down) as f32).floor() as i64;
                if error > 0 {
                    if error > end - now {
                        std::thread::sleep(Duration::from_millis((end - now) as u64));
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(error as u64));
                }
                now = uptime_ms();
                let alpha = (now - down) as f32 / duration as f32;
                inject(
                    host,
                    uid,
                    ACTION_MOVE,
                    down,
                    now,
                    x1 + (x2 - x1) * alpha,
                    y1 + (y2 - y1) * alpha,
                    1.0,
                )?;
                injected += 1;
                now = uptime_ms();
            }
            inject(host, uid, ACTION_UP, down, now, x2, y2, 0.0)
        }
        _ => Err(unimplemented(host, &format!("input {arg}"))),
    }
}

fn unimplemented(host: &AppHost, what: &str) -> String {
    let e = host.unimplemented_named("input", &format!("{what} (#1678)"));
    format!("java.lang.UnsupportedOperationException: {}", e.message)
}

/// `injectMotionEvent` into the default display, as
/// `InputManagerService.injectInputEvent` takes it.
#[allow(clippy::too_many_arguments)]
fn inject(
    host: &AppHost,
    uid: i32,
    action: i32,
    down_ms: i64,
    when_ms: i64,
    x: f32,
    y: f32,
    pressure: f32,
) -> Result<(), String> {
    let thrown = |e: Exception| {
        format!(
            "java.lang.{}: {}",
            if e.code == aim_binder_host::parcel::EX_SECURITY {
                "SecurityException"
            } else {
                "IllegalStateException"
            },
            e.message
        )
    };
    if host
        .check_permission(Some(INJECT_EVENTS), uid)
        .map_err(thrown)?
        != 0
    {
        return Err(thrown(Exception::security(
            "Injecting input events requires the caller (or the source of the instrumentation, if any) to have the INJECT_EVENTS permission.",
        )));
    }
    host.call_agent(INJECT_MOTION, "injectMotion", |p| {
        p.write_i32(action);
        p.write_i64(down_ms * 1_000_000);
        p.write_i64(when_ms * 1_000_000);
        p.write_f32(x);
        p.write_f32(y);
        p.write_f32(pressure);
    })
    .map(|_| ())
    .map_err(thrown)
}

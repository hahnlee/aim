//! `cmd input` (`input` in the guest's shell): InputShellCommand at
//! `android-16.0.0_r1` for the touchscreen (`tap`, `swipe`, injected as
//! `injectMotionEvent` makes them) and the keyboard (`keyevent`, `text`,
//! as `sendKeyEvent` and `sendText` make them, the text through the
//! virtual keyboard's key character map), injected
//! (`InputManagerService.injectInputEvent`, with INJECT_EVENTS) through the
//! app host agent's dispatcher, which delivers them as it delivers the
//! devices'. Other sources and commands, and displays but the default
//! one, are not implemented (#1678). The injection does not wait for the
//! window to finish the event.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::time::Duration;

use aim_binder_host::parcel::Exception;

use super::AppHost;
use crate::shell::ShellCommand;

const INJECT_EVENTS: &str = "android.permission.INJECT_EVENTS";
/// The agent's `injectMotion(int action, long downTimeNanos, long
/// eventTimeNanos, float x, float y, float pressure)`.
const INJECT_MOTION: u32 = 7;
/// The agent's `injectKey(int action, int flags, int keyCode, int
/// scanCode, int metaState, int repeatCount, long downTimeNanos, long
/// eventTimeNanos, int source, int displayId)`.
const INJECT_KEY: u32 = 11;
/// `KeyEvent.ACTION_*`, `FLAG_LONG_PRESS`, `LAST_KEYCODE`.
const KEY_ACTION_DOWN: i32 = 0;
const KEY_ACTION_UP: i32 = 1;
const KEY_FLAG_LONG_PRESS: i32 = 0x80;
const LAST_KEYCODE: i32 = 337;
/// `InputDevice.SOURCE_KEYBOARD`; `Display.INVALID_DISPLAY`, a key's
/// default display.
const SOURCE_KEYBOARD: i32 = 0x101;
const INVALID_DISPLAY: i32 = -1;
/// `ViewConfiguration`'s defaults: the long press timeout (the secure
/// setting `long_press_timeout`) and `DOUBLE_TAP_MIN_TIME`.
const DEFAULT_LONG_PRESS_TIMEOUT_MS: i64 = 400;
const DOUBLE_TAP_MIN_TIME_MS: u64 = 40;
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
    let mut source = None;
    if SOURCES.contains(&arg.as_str()) {
        if arg != "touchscreen" && arg != "keyboard" {
            return Err(unimplemented(host, &format!("the {arg} source")));
        }
        source = Some(arg);
        arg = c.next_arg_required()?;
    }
    let mut display = None;
    if arg == "-d" {
        let value = c.next_arg_required()?;
        display = Some(match value.as_str() {
            "0" | "DEFAULT_DISPLAY" => 0,
            "INVALID_DISPLAY" | "-1" => INVALID_DISPLAY,
            _ => return Err(unimplemented(host, &format!("display {value}"))),
        });
        arg = c.next_arg_required()?;
    }
    let is_key = matches!(arg.as_str(), "keyevent" | "text");
    let default_source = if is_key { "keyboard" } else { "touchscreen" };
    if let Some(source) = source.filter(|s| s != default_source) {
        return Err(unimplemented(
            host,
            &format!("{arg} from the {source} source"),
        ));
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
        // `runKeyEvent`.
        "keyevent" => {
            let display = display.unwrap_or(INVALID_DISPLAY);
            let (mut long_press, mut double_tap) = (false, false);
            let (mut delay_ms, mut duration_ms) = (0i64, 0i64);
            let long = |c: &mut ShellCommand| -> Result<i64, String> {
                let v = c.next_arg_required()?;
                v.parse::<i64>().map_err(|_| {
                    format!("java.lang.IllegalArgumentException: {INVALID_ARGUMENTS}keyevent")
                })
            };
            let mut next = Some(c.next_arg_required()?);
            while let Some(a) = next.clone().filter(|a| a.starts_with("--")) {
                long_press |= a == "--longpress";
                double_tap |= a == "--doubletap";
                if a == "--delay" {
                    delay_ms = long(c)?;
                } else if a == "--duration" {
                    duration_ms = long(c)?;
                }
                next = c.next_arg();
            }
            // Options alone leave no key: `keyCodeFromString(null)`.
            let Some(arg) = next else {
                return Err("java.lang.NullPointerException".into());
            };
            if duration_ms > 0 && long_press {
                c.eprintln("--duration and --longpress cannot be used at the same time.");
                return Err(
                    "java.lang.IllegalArgumentException: keyevent args should only contain either durationMs or longPress".into(),
                );
            }
            let long_press_timeout = long_press_timeout(host);
            if long_press {
                duration_ms = long_press_timeout;
            }
            let mut first = true;
            let mut key = Some(arg);
            while let Some(name) = key {
                if !first && delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(delay_ms as u64));
                }
                first = false;
                let key_code = key_code_from_string(&name);
                send_key_event(
                    host,
                    uid,
                    key_code,
                    duration_ms,
                    long_press_timeout,
                    display,
                )?;
                if double_tap {
                    std::thread::sleep(Duration::from_millis(DOUBLE_TAP_MIN_TIME_MS));
                    send_key_event(
                        host,
                        uid,
                        key_code,
                        duration_ms,
                        long_press_timeout,
                        display,
                    )?;
                }
                key = c.next_arg();
            }
            Ok(())
        }
        // `runText`, `sendText`: `%s` is a space.
        "text" => {
            let display = display.unwrap_or(INVALID_DISPLAY);
            let text = c.next_arg_required()?;
            let mut chars: Vec<u16> = text.encode_utf16().collect();
            let mut escape = false;
            let mut i = 0;
            while i < chars.len() {
                if escape {
                    escape = false;
                    if chars[i] == u16::from(b's') {
                        chars[i] = u16::from(b' ');
                        chars.remove(i - 1);
                        i -= 1;
                    }
                }
                if chars[i] == u16::from(b'%') {
                    escape = true;
                }
                i += 1;
            }
            let kcm = super::input_manager::virtual_key_character_map(host)
                .map_err(|e| format!("java.lang.IllegalStateException: {}", e.message))?;
            // `getEvents` fails for a character the map has no key for: no
            // events.
            let events = kcm.get_events(&chars).unwrap_or_default();
            let now = uptime_ms();
            for (key_code, meta_state, down) in events {
                let action = if down { KEY_ACTION_DOWN } else { KEY_ACTION_UP };
                inject_key(
                    host, uid, action, 0, key_code, meta_state, 0, now, now, display,
                )?;
            }
            Ok(())
        }
        _ => Err(unimplemented(host, &format!("input {arg}"))),
    }
}

/// `ViewConfiguration.getLongPressTimeout`: the secure setting, else the
/// default.
fn long_press_timeout(host: &AppHost) -> i64 {
    host.inputs()
        .ok()
        .and_then(|i| {
            i.settings
                .get(super::settings::Table::Secure, "long_press_timeout")
        })
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_LONG_PRESS_TIMEOUT_MS)
}

/// `KeyEvent.keyCodeFromString`: a number in range, else a label with or
/// without `KEYCODE_`, else `KEYCODE_UNKNOWN`.
fn key_code_from_string(name: &str) -> i32 {
    if let Ok(code) = name.parse::<i32>()
        && (0..=LAST_KEYCODE).contains(&code)
    {
        return code;
    }
    let label = name.strip_prefix("KEYCODE_").unwrap_or(name);
    aim_input_transport::keychars::key_code_by_label(label)
        .filter(|code| (0..=LAST_KEYCODE).contains(code))
        .unwrap_or(0)
}

/// `sendKeyEvent`: a down, a long press repeat after the long press
/// timeout if held that long, and an up.
fn send_key_event(
    host: &AppHost,
    uid: i32,
    key_code: i32,
    duration_ms: i64,
    long_press_timeout: i64,
    display: i32,
) -> Result<(), String> {
    let now = uptime_ms();
    inject_key(
        host,
        uid,
        KEY_ACTION_DOWN,
        0,
        key_code,
        0,
        0,
        now,
        now,
        display,
    )?;
    let first_sleep = duration_ms.min(long_press_timeout);
    if first_sleep > 0 {
        std::thread::sleep(Duration::from_millis(first_sleep as u64));
        if duration_ms >= long_press_timeout {
            let at = now + long_press_timeout;
            inject_key(
                host,
                uid,
                KEY_ACTION_DOWN,
                KEY_FLAG_LONG_PRESS,
                key_code,
                0,
                1,
                now,
                at,
                display,
            )?;
            let second_sleep = duration_ms - first_sleep;
            if second_sleep > 0 {
                std::thread::sleep(Duration::from_millis(second_sleep as u64));
            }
        }
    }
    // The down changed to an up, at the time it is sent.
    let up = uptime_ms();
    inject_key(
        host,
        uid,
        KEY_ACTION_UP,
        0,
        key_code,
        0,
        0,
        now,
        up,
        display,
    )
}

/// `injectInputEvent` of a key of the keyboard source.
#[allow(clippy::too_many_arguments)]
fn inject_key(
    host: &AppHost,
    uid: i32,
    action: i32,
    flags: i32,
    key_code: i32,
    meta_state: i32,
    repeat_count: i32,
    down_ms: i64,
    when_ms: i64,
    display: i32,
) -> Result<(), String> {
    check_inject(host, uid)?;
    host.call_agent(INJECT_KEY, "injectKey", |p| {
        p.write_i32(action);
        p.write_i32(flags);
        p.write_i32(key_code);
        p.write_i32(0); // scan code
        p.write_i32(meta_state);
        p.write_i32(repeat_count);
        p.write_i64(down_ms * 1_000_000);
        p.write_i64(when_ms * 1_000_000);
        p.write_i32(SOURCE_KEYBOARD);
        p.write_i32(display);
    })
    .map(|_| ())
    .map_err(thrown)
}

fn unimplemented(host: &AppHost, what: &str) -> String {
    let e = host.unimplemented_named("input", &format!("{what} (#1678)"));
    format!("java.lang.UnsupportedOperationException: {}", e.message)
}

/// What the shell command throws of an exception.
fn thrown(e: Exception) -> String {
    format!(
        "java.lang.{}: {}",
        if e.code == aim_binder_host::parcel::EX_SECURITY {
            "SecurityException"
        } else {
            "IllegalStateException"
        },
        e.message
    )
}

/// `injectInputEvent`'s INJECT_EVENTS check.
fn check_inject(host: &AppHost, uid: i32) -> Result<(), String> {
    if host
        .check_permission(Some(INJECT_EVENTS), uid)
        .map_err(thrown)?
        != 0
    {
        return Err(thrown(Exception::security(
            "Injecting input events requires the caller (or the source of the instrumentation, if any) to have the INJECT_EVENTS permission.",
        )));
    }
    Ok(())
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
    check_inject(host, uid)?;
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

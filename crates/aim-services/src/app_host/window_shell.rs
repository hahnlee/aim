//! `cmd window` (`wm`): `WindowManagerShellCommand` at `android-16.0.0_r1`
//! over the app host's window service, run with the caller's identity as
//! `WindowManagerService.onShellCommand` runs it. The display's size and
//! density are its own (the app host applies no forced size, density or
//! scaling, so the base metrics are the initial ones), the folded area is
//! `config_foldedArea` or its override (`DisplayFoldController`), and the
//! keyguard is never shown. Overriding the display's metrics, rotation,
//! orientation requests, letterboxing, multi-window, blur, windowing mode,
//! tracing, logging and dumps: #2050.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Exception;

use super::AppHost;
use super::settings_service::java_exception;
use crate::shell::ShellCommand;

const WRITE_SECURE_SETTINGS: &str = "android.permission.WRITE_SECURE_SETTINGS";
const CONTROL_KEYGUARD: &str = "android.permission.CONTROL_KEYGUARD";
/// `Display.DEFAULT_DISPLAY`, `INVALID_DISPLAY`.
const DEFAULT_DISPLAY: i32 = 0;
const INVALID_DISPLAY: i32 = -1;
/// `DisplayMetrics.DENSITY_DEFAULT`.
const DENSITY_DEFAULT: i32 = 160;

/// What the command answers: its result, or the exception `exec` prints.
type Outcome = Result<i32, String>;

/// `exec` of `onCommand`.
pub(super) fn run(host: &AppHost, uid: i32, c: &mut ShellCommand) -> i32 {
    let Some(cmd) = c.command().map(str::to_string) else {
        // `handleDefaultCommands(null)`.
        help(host, c);
        return -1;
    };
    match on_command(host, uid, &cmd, c) {
        Ok(result) => result,
        Err(exception) => {
            c.exception(&exception);
            -1
        }
    }
}

fn on_command(host: &AppHost, uid: i32, cmd: &str, c: &mut ShellCommand) -> Outcome {
    let java = |e: Exception| java_exception(&e);
    match cmd {
        "size" => display_size(host, uid, c),
        "density" => display_density(host, uid, c),
        "folded-area" => folded_area(host, uid, c),
        "dismiss-keyguard" => {
            // `dismissKeyguard(null, null)`: with the keyguard not showing
            // and no callback, `dismissKeyguardLw` has nothing to do.
            if host
                .check_permission(Some(CONTROL_KEYGUARD), uid)
                .map_err(java)?
                != 0
            {
                return Err(
                    "java.lang.SecurityException: Requires CONTROL_KEYGUARD permission".into(),
                );
            }
            Ok(0)
        }
        "reset" => {
            let arg = c.next_arg();
            display_id(c, arg.as_deref())?;
            // `clearForcedDisplaySize`, the first of what it resets.
            enforce_write_secure_settings(host, uid)?;
            Err(java(host.unimplemented_named(
                "window",
                "wm reset: forced display size (#2050)",
            )))
        }
        "scaling" => {
            let mode = c.next_arg_required()?;
            if mode != "auto" && mode != "off" {
                c.eprintln("Error: scaling must be 'auto' or 'off'");
                return Ok(-1);
            }
            display_id(c, Some(&mode))?;
            // `setForcedDisplayScalingMode`'s @EnforcePermission.
            enforce_write_secure_settings(host, uid)?;
            Err(java(host.unimplemented_named(
                "window",
                "forced display scaling (#2050)",
            )))
        }
        "tracing"
        | "logging"
        | "user-rotation"
        | "fixed-to-user-rotation"
        | "set-ignore-orientation-request"
        | "get-ignore-orientation-request"
        | "dump-visible-window-views"
        | "set-letterbox-style"
        | "get-letterbox-style"
        | "reset-letterbox-style"
        | "set-sandbox-display-apis"
        | "set-multi-window-config"
        | "get-multi-window-config"
        | "reset-multi-window-config"
        | "disable-blur"
        | "reset-freeze-recent-tasks"
        | "set-display-windowing-mode"
        | "get-display-windowing-mode"
        | "shell"
        | "dump" => Err(java(
            host.unimplemented_named("window", &format!("wm {cmd} (#2050)")),
        )),
        // `handleDefaultCommands`.
        "help" | "-h" => {
            help(host, c);
            Ok(-1)
        }
        _ => {
            c.println(&format!("Unknown command: {cmd}"));
            Ok(-1)
        }
    }
}

/// `getDisplayId(opt)`: the display after `-d`, else the default; a bad
/// number is printed and the default kept.
fn display_id(c: &mut ShellCommand, opt: Option<&str>) -> Result<i32, String> {
    let option = match opt {
        Some("-d") => Some("-d".to_string()),
        _ => c.next_option()?,
    };
    if option.as_deref() != Some("-d") {
        return Ok(DEFAULT_DISPLAY);
    }
    match c.next_arg_required() {
        Ok(arg) => match parse_int(&arg) {
            Ok(id) => return Ok(id),
            Err(e) => c.eprintln(&format!("Error: bad number {e}")),
        },
        Err(e) => c.eprintln(&format!("Error: {e}")),
    }
    Ok(DEFAULT_DISPLAY)
}

/// `Integer.parseInt`, with its `NumberFormatException` as `toString`
/// prints it.
fn parse_int(s: &str) -> Result<i32, String> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "java.lang.NumberFormatException: For input string: \"{s}\""
        ));
    }
    s.parse()
        .map_err(|_| format!("java.lang.NumberFormatException: For input string: \"{s}\""))
}

/// `@EnforcePermission(WRITE_SECURE_SETTINGS)`.
fn enforce_write_secure_settings(host: &AppHost, uid: i32) -> Result<(), String> {
    if host
        .check_permission(Some(WRITE_SECURE_SETTINGS), uid)
        .map_err(|e| java_exception(&e))?
        != 0
    {
        return Err(format!(
            "java.lang.SecurityException: Access denied, requires: {WRITE_SECURE_SETTINGS}"
        ));
    }
    Ok(())
}

/// A `Point`: x, y.
type Point = (i32, i32);

/// `getInitialDisplaySize` and `getBaseDisplaySize`: the display's own
/// size for the default display (no forced size is applied), else the
/// zero point the caller passed.
fn sizes(host: &AppHost, display: i32) -> Result<(Point, Point), String> {
    if display != DEFAULT_DISPLAY {
        return Ok(((0, 0), (0, 0)));
    }
    let d = host.display().map_err(|e| java_exception(&e))?;
    let size = (d.0.width, d.0.height);
    Ok((size, size))
}

/// `getInitialDisplayDensity` and `getBaseDisplayDensity`: the display's
/// logical density for the default display (no forced density is
/// applied), -1 for a display that does not exist.
fn densities(host: &AppHost, display: i32) -> Result<(i32, i32), String> {
    if display != DEFAULT_DISPLAY {
        return Ok((-1, -1));
    }
    let d = host.display().map_err(|e| java_exception(&e))?;
    let density = d.0.density_dpi();
    Ok((density, density))
}

/// `runDisplaySize`.
fn display_size(host: &AppHost, uid: i32, c: &mut ShellCommand) -> Outcome {
    let size = c.next_arg();
    let display = display_id(c, size.as_deref())?;
    let Some(size) = size.filter(|s| s != "-d") else {
        let (initial, base) = sizes(host, display)?;
        c.println(&format!("Physical size: {}x{}", initial.0, initial.1));
        if initial != base {
            c.println(&format!("Override size: {}x{}", base.0, base.1));
        }
        return Ok(0);
    };
    let (w, h) = if size == "reset" {
        (-1, -1)
    } else {
        let Some(div) = size.find('x').filter(|&d| d > 0 && d < size.len() - 1) else {
            c.eprintln(&format!("Error: bad size {size}"));
            return Ok(-1);
        };
        match (
            dimension(host, &size[..div], display),
            dimension(host, &size[div + 1..], display),
        ) {
            (Ok(w), Ok(h)) => (w?, h?),
            (Err(e), _) | (_, Err(e)) => {
                c.eprintln(&format!("Error: bad number {e}"));
                return Ok(-1);
            }
        }
    };
    // `setForcedDisplaySize` or `clearForcedDisplaySize`.
    enforce_write_secure_settings(host, uid)?;
    let name = if w >= 0 && h >= 0 {
        "forced display size (#2050)"
    } else {
        "clearing the forced display size (#2050)"
    };
    Err(java_exception(&host.unimplemented_named("window", name)))
}

/// `parseDimension`: pixels, or dp at the display's base density. The
/// outer error is the number's; the inner one an exception reading the
/// density.
fn dimension(host: &AppHost, s: &str, display: i32) -> Result<Result<i32, String>, String> {
    if let Some(px) = s.strip_suffix("px") {
        return parse_int(px).map(Ok);
    }
    if let Some(dp) = s.strip_suffix("dp") {
        let dp = parse_int(dp)?;
        return Ok(
            densities(host, display).map(|(_, base)| dp.wrapping_mul(base) / DENSITY_DEFAULT)
        );
    }
    parse_int(s).map(Ok)
}

/// `runDisplayDensity`.
fn display_density(host: &AppHost, uid: i32, c: &mut ShellCommand) -> Outcome {
    let mut arg = c.next_arg();
    let mut density_str = None;
    let mut display = DEFAULT_DISPLAY;
    if !matches!(arg.as_deref(), Some("-d" | "-u")) {
        density_str = arg;
        arg = c.next_arg();
    }
    match arg.as_deref() {
        Some("-d") => {
            let id = c.next_arg();
            match id.as_deref().map(parse_int) {
                Some(Ok(id)) => display = id,
                Some(Err(e)) => c.eprintln(&format!("Error: bad number {e}")),
                None => c.eprintln(
                    "Error: bad number java.lang.NumberFormatException: Cannot parse null string: null",
                ),
            }
        }
        Some("-u") => {
            // `getDisplayIdByUniqueId`: the built-in display's
            // `DisplayInfo.uniqueId`.
            let unique = c.next_arg();
            let d = host.display().map_err(|e| java_exception(&e))?;
            display = if unique.as_deref() == Some(&format!("local:{}", d.0.id)) {
                DEFAULT_DISPLAY
            } else {
                INVALID_DISPLAY
            };
            if display == INVALID_DISPLAY {
                c.eprintln("Error: the uniqueId is invalid ");
                return Ok(-1);
            }
        }
        _ => {}
    }
    let Some(density_str) = density_str else {
        let (initial, base) = densities(host, display)?;
        c.println(&format!("Physical density: {initial}"));
        if initial != base {
            c.println(&format!("Override density: {base}"));
        }
        return Ok(0);
    };
    let density = if density_str == "reset" {
        -1
    } else {
        match parse_int(&density_str) {
            Ok(d) if d < 72 => {
                c.eprintln("Error: density must be >= 72");
                return Ok(-1);
            }
            Ok(d) => d,
            Err(e) => {
                c.eprintln(&format!("Error: bad number {e}"));
                return Ok(-1);
            }
        }
    };
    // `setForcedDisplayDensityForUser` or `clearForcedDisplayDensityForUser`
    // for USER_CURRENT, the app host's only user.
    enforce_write_secure_settings(host, uid)?;
    let name = if density > 0 {
        "forced display density (#2050)"
    } else {
        "clearing the forced display density (#2050)"
    };
    Err(java_exception(&host.unimplemented_named("window", name)))
}

/// A `Rect`: left, top, right, bottom.
type Rect = [i32; 4];

/// `Rect.isEmpty`.
fn empty(r: &Rect) -> bool {
    r[0] >= r[2] || r[1] >= r[3]
}

/// `Rect.unflattenFromString` of `config_foldedArea`, an empty one for
/// none (`DisplayFoldController.create`): its four integers separated by
/// single spaces, else null, which `getFoldedArea` then dereferences.
fn unflatten(s: &str) -> Result<Rect, String> {
    let parts: Vec<&str> = s.split(' ').collect();
    let null = || "java.lang.NullPointerException".to_string();
    if parts.len() != 4 {
        return Err(null());
    }
    let mut rect = [0; 4];
    for (slot, part) in rect.iter_mut().zip(parts) {
        let digits = part.strip_prefix('-').unwrap_or(part);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(null());
        }
        *slot = parse_int(part)?;
    }
    Ok(rect)
}

/// `DisplayFoldController.getFoldedArea`: its override, else the
/// configured area.
fn folded(host: &AppHost) -> Result<Rect, String> {
    let r#override = *host.windows.folded_area_override.lock().unwrap();
    if !empty(&r#override) {
        return Ok(r#override);
    }
    let inputs = host.inputs().map_err(|e| java_exception(&e))?;
    match inputs.packages.framework_string("config_foldedArea") {
        Some(s) if !s.is_empty() => unflatten(&s),
        _ => Ok([0; 4]),
    }
}

/// `runDisplayFoldedArea`.
fn folded_area(host: &AppHost, uid: i32, c: &mut ShellCommand) -> Outcome {
    let rect = match c.next_arg() {
        None => {
            let area = folded(host)?;
            if empty(&area) {
                c.println("Folded area: none");
            } else {
                c.println(&format!(
                    "Folded area: {},{},{},{}",
                    area[0], area[1], area[2], area[3]
                ));
            }
            return Ok(0);
        }
        Some(s) if s == "reset" => [0; 4],
        Some(s) => {
            // `(-?\d+),(-?\d+),(-?\d+),(-?\d+)`, matched whole.
            let parts: Vec<&str> = s.split(',').collect();
            let matches = parts.len() == 4
                && parts.iter().all(|p| {
                    let d = p.strip_prefix('-').unwrap_or(p);
                    !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
                });
            if !matches {
                c.eprintln("Error: area should be LEFT,TOP,RIGHT,BOTTOM");
                return Ok(-1);
            }
            let mut rect = [0; 4];
            for (slot, part) in rect.iter_mut().zip(parts) {
                *slot = parse_int(part)?;
            }
            rect
        }
    };
    // `setOverrideFoldedArea`.
    if host
        .check_permission(Some(WRITE_SECURE_SETTINGS), uid)
        .map_err(|e| java_exception(&e))?
        != 0
    {
        return Err(format!(
            "java.lang.SecurityException: Must hold permission {WRITE_SECURE_SETTINGS}"
        ));
    }
    *host.windows.folded_area_override.lock().unwrap() = rect;
    Ok(0)
}

/// `onHelp`; the tracing and logging verbs only on a build that is not
/// `user` (`Build.IS_USER`).
fn help(host: &AppHost, c: &mut ShellCommand) {
    c.print(HELP);
    if host.property("ro.build.type").as_deref() != Some("user") {
        c.print(HELP_DEBUG);
    }
}

const HELP: &str = "\
Window manager (window) commands:
  help
      Print this help text.
  size [reset|WxH|WdpxHdp] [-d DISPLAY_ID]
    Return or override display size.
    width and height in pixels unless suffixed with 'dp'.
  density [reset|DENSITY] [-d DISPLAY_ID] [-u UNIQUE_ID]
    Return or override display density.
  folded-area [reset|LEFT,TOP,RIGHT,BOTTOM]
    Return or override folded area.
  scaling [off|auto] [-d DISPLAY_ID]
    Set display scaling mode.
  dismiss-keyguard
    Dismiss the keyguard, prompting user for auth if necessary.
  disable-blur [true|1|false|0]
  user-rotation [-d DISPLAY_ID] [free|lock] [rotation]
    Print or set user rotation mode and user rotation.
  dump-visible-window-views
    Dumps the encoded view hierarchies of visible windows
  fixed-to-user-rotation [-d DISPLAY_ID] [enabled|disabled|default
      |enabled_if_no_auto_rotation]
    Print or set rotating display for app requested orientation.
  set-ignore-orientation-request [-d DISPLAY_ID] [reset|true|1|false|0]
  get-ignore-orientation-request [-d DISPLAY_ID]
    If app requested orientation should be ignored.
  set-sandbox-display-apis [true|1|false|0]
    Sets override of Display APIs getRealSize / getRealMetrics to reflect
    DisplayArea of the activity, or the window bounds if in letterbox or
    Size Compat Mode.
  set-letterbox-style
    Sets letterbox style using the following options:
      --aspectRatio aspectRatio
        Aspect ratio of letterbox for fixed orientation. If aspectRatio <= 1.0
        both it and R.dimen.config_fixedOrientationLetterboxAspectRatio will
        be ignored and framework implementation will determine aspect ratio.
      --minAspectRatioForUnresizable aspectRatio
        Default min aspect ratio for unresizable apps which is used when an
        app is eligible for the size compat mode.  If aspectRatio <= 1.0
        both it and R.dimen.config_fixedOrientationLetterboxAspectRatio will
        be ignored and framework implementation will determine aspect ratio.
      --cornerRadius radius
        Corners radius (in pixels) for activities in the letterbox mode.
        If radius < 0, both R.integer.config_letterboxActivityCornersRadius
        and it will be ignored and corners of the activity won't be rounded.
      --backgroundType [reset|solid_color|app_color_background
          |app_color_background_floating|wallpaper]
        Type of background used in the letterbox mode.
      --backgroundColor color
        Color of letterbox which is be used when letterbox background type
        is 'solid-color'. Use (set)get-letterbox-style to check and control
        letterbox background type. See Color#parseColor for allowed color
        formats (#RRGGBB and some colors by name, e.g. magenta or olive).
      --backgroundColorResource resource_name
        Color resource name of letterbox background which is used when
        background type is 'solid-color'. Use (set)get-letterbox-style to
        check and control background type. Parameter is a color resource
        name, for example, @android:color/system_accent2_50.
      --wallpaperBlurRadius radius
        Blur radius for 'wallpaper' letterbox background. If radius <= 0
        both it and R.dimen.config_letterboxBackgroundWallpaperBlurRadius
        are ignored and 0 is used.
      --wallpaperDarkScrimAlpha alpha
        Alpha of a black translucent scrim shown over 'wallpaper'
        letterbox background. If alpha < 0 or >= 1 both it and
        R.dimen.config_letterboxBackgroundWallaperDarkScrimAlpha are ignored
        and 0.0 (transparent) is used instead.
      --horizontalPositionMultiplier multiplier
        Horizontal position of app window center. If multiplier < 0 or > 1,
        both it and R.dimen.config_letterboxHorizontalPositionMultiplier
        are ignored and central position (0.5) is used.
      --verticalPositionMultiplier multiplier
        Vertical position of app window center. If multiplier < 0 or > 1,
        both it and R.dimen.config_letterboxVerticalPositionMultiplier
        are ignored and central position (0.5) is used.
      --isHorizontalReachabilityEnabled [true|1|false|0]
        Whether horizontal reachability repositioning is allowed for
        letterboxed fullscreen apps in landscape device orientation.
      --isVerticalReachabilityEnabled [true|1|false|0]
        Whether vertical reachability repositioning is allowed for
        letterboxed fullscreen apps in portrait device orientation.
      --defaultPositionForHorizontalReachability [left|center|right]
        Default position of app window when horizontal reachability is.
        enabled.
      --defaultPositionForVerticalReachability [top|center|bottom]
        Default position of app window when vertical reachability is.
        enabled.
      --persistentPositionForHorizontalReachability [left|center|right]
        Persistent position of app window when horizontal reachability is.
        enabled.
      --persistentPositionForVerticalReachability [top|center|bottom]
        Persistent position of app window when vertical reachability is.
        enabled.
      --isEducationEnabled [true|1|false|0]
        Whether education is allowed for letterboxed fullscreen apps.
      --isSplitScreenAspectRatioForUnresizableAppsEnabled [true|1|false|0]
        Whether using split screen aspect ratio as a default aspect ratio for
        unresizable apps.
      --isTranslucentLetterboxingEnabled [true|1|false|0]
        Whether letterboxing for translucent activities is enabled.
      --isUserAppAspectRatioSettingsEnabled [true|1|false|0]
        Whether user aspect ratio settings are enabled.
      --isUserAppAspectRatioFullscreenEnabled [true|1|false|0]
        Whether user aspect ratio fullscreen option is enabled.
      --isCameraCompatRefreshEnabled [true|1|false|0]
        Whether camera compatibility refresh is enabled.
      --isCameraCompatRefreshCycleThroughStopEnabled [true|1|false|0]
        Whether activity \"refresh\" in camera compatibility treatment should
        happen using the \"stopped -> resumed\" cycle rather than
        \"paused -> resumed\" cycle.
      --cameraCompatAspectRatio aspectRatio
        Aspect ratio of letterbox for fixed-orientation camera apps, during
        freeform camera compat mode. If aspectRatio <= 1.0
        it will be ignored.
      --isCameraCompatFreeformWindowingTreatmentEnabled [true|1|false|0]
        Whether camera compat treatment is enabled in freeform mode for all
        eligible apps.
  reset-letterbox-style [aspectRatio|cornerRadius|backgroundType
      |backgroundColor|wallpaperBlurRadius|wallpaperDarkScrimAlpha
      |horizontalPositionMultiplier|verticalPositionMultiplier
      |isHorizontalReachabilityEnabled|isVerticalReachabilityEnabled
      |isEducationEnabled|defaultPositionMultiplierForHorizontalReachability
      |isTranslucentLetterboxingEnabled|isUserAppAspectRatioSettingsEnabled
      |persistentPositionMultiplierForHorizontalReachability
      |persistentPositionMultiplierForVerticalReachability
      |defaultPositionMultiplierForVerticalReachability
      |cameraCompatAspectRatio
      |isCameraCompatFreeformWindowingTreatmentEnabled]
    Resets overrides to default values for specified properties separated
    by space, e.g. 'reset-letterbox-style aspectRatio cornerRadius'.
    If no arguments provided, all values will be reset.
  get-letterbox-style
    Prints letterbox style configuration.
  set-multi-window-config
    Sets options to determine if activity should be shown in multi window:
      --supportsNonResizable [configValue]
        Whether the device supports non-resizable activity in multi window.
        -1: The device doesn't support non-resizable in multi window.
         0: The device supports non-resizable in multi window only if
            this is a large screen device.
         1: The device always supports non-resizable in multi window.
      --respectsActivityMinWidthHeight [configValue]
        Whether the device checks the activity min width/height to determine
        if it can be shown in multi window.
        -1: The device ignores the activity min width/height when determining
            if it can be shown in multi window.
         0: If this is a small screen, the device compares the activity min
            width/height with the min multi window modes dimensions
            the device supports to determine if the activity can be shown in
            multi window.
         1: The device always compare the activity min width/height with the
            min multi window dimensions the device supports to determine if
            the activity can be shown in multi window.
  get-multi-window-config
    Prints values of the multi window config options.
  reset-multi-window-config
    Resets overrides to default values of the multi window config options.
  reset-freeze-recent-tasks
    Resets the spatial ordering of the recent tasks list
  set-display-windowing-mode [-d DISPLAY_ID] [mode_id]
    As mode_id, use 0 for undefined, 5 for freeform, 1 for fullscreen
  get-display-windowing-mode [-d DISPLAY_ID]
  reset [-d DISPLAY_ID]
    Reset all override settings.
  shell <cmd> ...
    Runs a WMShell command.  To see a full list of available wmshell commands run 'adb shell wm shell help'
";

const HELP_DEBUG: &str = "\
  tracing (start | stop)
    Start or stop window tracing.
  logging (start | stop | enable | disable | enable-text | disable-text)
    Logging settings.
";

#[cfg(test)]
mod tests {
    use super::*;

    /// `Integer.parseInt` and its exception's text.
    #[test]
    fn numbers_parse_as_java_parses_them() {
        assert_eq!(parse_int("1080"), Ok(1080));
        assert_eq!(parse_int("+5"), Ok(5));
        assert_eq!(parse_int("-3"), Ok(-3));
        assert_eq!(
            parse_int("12a"),
            Err("java.lang.NumberFormatException: For input string: \"12a\"".into())
        );
        assert_eq!(
            parse_int(""),
            Err("java.lang.NumberFormatException: For input string: \"\"".into())
        );
        assert!(parse_int("99999999999").is_err());
    }

    /// `Rect.unflattenFromString` and `isEmpty`.
    #[test]
    fn folded_area_config_unflattens() {
        assert_eq!(unflatten("0 100 1080 1200"), Ok([0, 100, 1080, 1200]));
        assert!(unflatten("0,100,1080,1200").is_err());
        assert!(unflatten("0 100 1080").is_err());
        assert!(empty(&[0; 4]));
        assert!(empty(&[10, 0, 10, 5]));
        assert!(!empty(&[0, 100, 1080, 1200]));
    }
}

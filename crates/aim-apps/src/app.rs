//! What a launcher shows of an installed app, from its manifest: its
//! package, and the label and icon of each of its launcher activities.

use crate::apk::{ATTR_THEME, Apk, Resources};
use crate::icon::Icon;
use crate::res::{Element, Result, Value};
use crate::restrictions;

// `android:attr` resource ids.
const ATTR_LABEL: u32 = 0x0101_0001;
const ATTR_ICON: u32 = 0x0101_0002;
const ATTR_NAME: u32 = 0x0101_0003;
const ATTR_ENABLED: u32 = 0x0101_000e;
const ATTR_VERSION_CODE: u32 = 0x0101_021b;
const ATTR_TARGET_ACTIVITY: u32 = 0x0101_0202;

/// An entry a launcher lists: one launcher activity of an app.
#[derive(Clone, Debug)]
pub struct App {
    pub package: String,
    /// The launcher activity's label.
    pub label: String,
    /// The application's label.
    pub app_label: String,
    /// The package's primary entry: the first named as the application,
    /// else its first (in the manifest's order).
    pub primary: bool,
    pub version: i64,
    /// The launcher activity (or alias), its full class name.
    pub activity: String,
    /// The launcher activity's icon, else the application's.
    pub icon: Option<Value>,
    pub theme: u32,
}

/// `android:enabled`, which may be a (configuration-dependent) resource.
fn enabled(res: &Resources, e: &Element) -> bool {
    let v = match e.attr(ATTR_ENABLED) {
        Some(Value::Ref(id)) => res.resolve(&Value::Ref(*id)).map(|(v, _)| v),
        v => v.cloned(),
    };
    !matches!(v, Some(Value::Bool(false)))
}

/// A component's class name, made full (`.Main` is `package.Main`).
fn full_name(package: &str, n: &str) -> String {
    if n.starts_with('.') {
        format!("{package}{n}")
    } else if !n.contains('.') {
        format!("{package}.{n}")
    } else {
        n.to_string()
    }
}

/// An element's `android:name`, made full.
fn class_name(package: &str, e: &Element) -> String {
    match e.attr(ATTR_NAME) {
        Some(Value::String(n)) => full_name(package, n),
        _ => String::new(),
    }
}

/// Whether an `<activity>` or `<activity-alias>` has a MAIN / LAUNCHER
/// intent filter.
fn launcher(e: &Element) -> bool {
    let named =
        |c: &Element, what: &str| matches!(c.attr(ATTR_NAME), Some(Value::String(s)) if s == what);
    e.children
        .iter()
        .filter(|f| f.name == "intent-filter")
        .any(|f| {
            f.children
                .iter()
                .any(|c| c.name == "action" && named(c, "android.intent.action.MAIN"))
                && f.children
                    .iter()
                    .any(|c| c.name == "category" && named(c, "android.intent.category.LAUNCHER"))
        })
}

/// The launcher entries of `apk`, one per enabled launcher activity (or
/// alias), in the manifest's order, as a launcher lists them: each with
/// its own label and icon, else its target activity's (an alias's), else
/// the application's. None when the app is disabled or has no launcher
/// activity. `framework` resolves `@android:` references; `state` is the
/// user's (components enabled or disabled at run time).
pub fn read(
    apk: &Apk,
    framework: Option<&Apk>,
    state: Option<&restrictions::Package>,
) -> Result<Vec<App>> {
    let manifest = apk.manifest()?;
    let Some(Value::String(package)) = manifest.named("package") else {
        return Ok(Vec::new());
    };
    let Some(application) = manifest.children.iter().find(|c| c.name == "application") else {
        return Ok(Vec::new());
    };
    let theme = match application.attr(ATTR_THEME) {
        Some(Value::Ref(id)) => *id,
        _ => 0,
    };
    let res = Resources {
        app: apk,
        framework,
        theme,
    };
    if !enabled(&res, application) {
        return Ok(Vec::new());
    }
    let on = |c: &Element| {
        let name = class_name(package, c);
        match state {
            Some(s) if s.disabled_components.contains(&name) => false,
            Some(s) if s.enabled_components.contains(&name) => true,
            _ => enabled(&res, c),
        }
    };
    let components: Vec<&Element> = application
        .children
        .iter()
        .filter(|c| c.name == "activity" || c.name == "activity-alias")
        .collect();
    // An alias's target activity, whose label and icon it inherits.
    let target = |alias: &Element| {
        let Some(Value::String(t)) = alias.attr(ATTR_TARGET_ACTIVITY) else {
            return None;
        };
        let t = full_name(package, t);
        components
            .iter()
            .find(|c| c.name == "activity" && class_name(package, c) == t)
            .copied()
    };
    let app_label = application
        .attr(ATTR_LABEL)
        .and_then(|v| res.string(v))
        .unwrap_or_else(|| package.clone());
    let version = match manifest.attr(ATTR_VERSION_CODE) {
        Some(Value::Int(v)) => *v as i64,
        _ => 0,
    };
    let mut apps = Vec::new();
    for activity in components.iter().filter(|c| on(c) && launcher(c)) {
        let chain = [Some(*activity), target(activity)];
        let label = chain
            .iter()
            .flatten()
            .filter_map(|c| c.attr(ATTR_LABEL))
            .find_map(|v| res.string(v))
            .unwrap_or_else(|| app_label.clone());
        let icon = chain
            .iter()
            .flatten()
            .find_map(|c| c.attr(ATTR_ICON))
            .or(application.attr(ATTR_ICON))
            .cloned();
        let activity = class_name(package, activity);
        // A class listed twice (an activity and an alias of one name) is
        // one entry.
        if apps.iter().any(|a: &App| a.activity == activity) {
            continue;
        }
        apps.push(App {
            package: package.clone(),
            label,
            app_label: app_label.clone(),
            primary: false,
            version,
            activity,
            icon,
            theme,
        });
    }
    // The one named as the application, else the first.
    let primary = apps.iter().position(|a| a.label == app_label).unwrap_or(0);
    if let Some(a) = apps.get_mut(primary) {
        a.primary = true;
    }
    Ok(apps)
}

/// The platform itself (`framework-res.apk`, package `android`): its
/// application's label and icon, and no activity. Its shim shows the
/// notifications of packages that have no shim of their own
/// (`docs/notifications.md`).
pub fn system(framework: &Apk) -> Result<Option<App>> {
    let manifest = framework.manifest()?;
    let Some(Value::String(package)) = manifest.named("package") else {
        return Ok(None);
    };
    let Some(application) = manifest.children.iter().find(|c| c.name == "application") else {
        return Ok(None);
    };
    let theme = match application.attr(ATTR_THEME) {
        Some(Value::Ref(id)) => *id,
        _ => 0,
    };
    let res = Resources {
        app: framework,
        framework: None,
        theme,
    };
    let label = application
        .attr(ATTR_LABEL)
        .and_then(|v| res.string(v))
        .unwrap_or_else(|| package.clone());
    let version = match manifest.attr(ATTR_VERSION_CODE) {
        Some(Value::Int(v)) => *v as i64,
        _ => 0,
    };
    Ok(Some(App {
        package: package.clone(),
        app_label: label.clone(),
        primary: true,
        label,
        version,
        activity: String::new(),
        icon: application.attr(ATTR_ICON).cloned(),
        theme,
    }))
}

impl App {
    /// The app's icon as a macOS `.icns`.
    pub fn icns(&self, apk: &Apk, framework: Option<&Apk>) -> Option<Vec<u8>> {
        let res = Resources {
            app: apk,
            framework,
            theme: self.theme,
        };
        let icon = Icon::of(&res, self.icon.as_ref()?)?;
        crate::icon::icns(&res, &icon)
    }
}

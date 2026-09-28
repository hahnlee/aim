//! What a launcher shows of an installed app, from its manifest: its
//! package, the label and icon of its launcher activity.

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

/// An app a launcher lists.
#[derive(Clone, Debug)]
pub struct App {
    pub package: String,
    pub label: String,
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
fn class_name(package: &str, e: &Element) -> String {
    match e.attr(ATTR_NAME) {
        Some(Value::String(n)) if n.starts_with('.') => format!("{package}{n}"),
        Some(Value::String(n)) if !n.contains('.') => format!("{package}.{n}"),
        Some(Value::String(n)) => n.clone(),
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

/// The app in `apk`, if a launcher lists it (it has an enabled launcher
/// activity). `framework` resolves `@android:` references; `state` is the
/// user's (components enabled or disabled at run time).
pub fn read(
    apk: &Apk,
    framework: Option<&Apk>,
    state: Option<&restrictions::Package>,
) -> Result<Option<App>> {
    let manifest = apk.manifest()?;
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
        app: apk,
        framework,
        theme,
    };
    if !enabled(&res, application) {
        return Ok(None);
    }
    let on = |c: &Element| {
        let name = class_name(package, c);
        match state {
            Some(s) if s.disabled_components.contains(&name) => false,
            Some(s) if s.enabled_components.contains(&name) => true,
            _ => enabled(&res, c),
        }
    };
    let Some(activity) = application
        .children
        .iter()
        .filter(|c| c.name == "activity" || c.name == "activity-alias")
        .find(|c| on(c) && launcher(c))
    else {
        return Ok(None);
    };
    let label = [activity.attr(ATTR_LABEL), application.attr(ATTR_LABEL)]
        .into_iter()
        .flatten()
        .find_map(|v| res.string(v))
        .unwrap_or_else(|| package.clone());
    let icon = activity
        .attr(ATTR_ICON)
        .or(application.attr(ATTR_ICON))
        .cloned();
    let version = match manifest.attr(ATTR_VERSION_CODE) {
        Some(Value::Int(v)) => *v as i64,
        _ => 0,
    };
    Ok(Some(App {
        package: package.clone(),
        label,
        version,
        activity: class_name(package, activity),
        icon,
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

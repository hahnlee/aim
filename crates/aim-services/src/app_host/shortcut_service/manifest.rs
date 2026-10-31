//! `ShortcutParser` and `Intent.parseIntent` at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::*;
use crate::package::intent::Intent;
use crate::package::parse::{attrs::Ctx, resources::*};
use aim_apps::res::Element as ManifestElement;

#[derive(Default)]
pub(super) struct Manifest {
    pub shortcuts: Vec<Shortcut>,
    pub share_targets: Vec<Element>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.is_empty())
}

/// Intents keep the exact target class (ComponentName's constructor does
/// not expand it). Float extras parse, but cannot enter PersistableBundle.
fn intent(
    e: &ManifestElement,
    ctx: &Ctx<'_>,
) -> Result<(FullIntent, Option<Vec<(String, Value)>>), String> {
    let a = ctx.obtain(e);
    let mut intent = Intent {
        action: a.string("action"),
        data: a
            .string("data")
            .map(|s| crate::package::uri::Uri::parse(&s)),
        ty: a.string("mimeType"),
        identifier: a.string("identifier"),
        component: a
            .string("targetPackage")
            .zip(a.string("targetClass"))
            .map(|(package, class)| ComponentName { package, class }),
        ..Default::default()
    };
    let mut extras = None;
    let mut categories = Vec::new();
    for c in &e.children {
        let a = ctx.obtain(c);
        match c.name.as_str() {
            "categories" => {
                if let Some(name) = a.string("name")
                    && !categories.contains(&name)
                {
                    categories.push(name);
                }
            }
            "extra" => {
                let name = a.string("name").ok_or("intent extra without name")?;
                let v = a.peek("value").ok_or("intent extra without value")?;
                let value = match v.kind {
                    TYPE_STRING => Value::String(v.string),
                    TYPE_INT_BOOLEAN => Value::Bool(v.data != 0),
                    TYPE_FIRST_INT..=TYPE_LAST_INT => Value::Int(v.data as i32),
                    TYPE_FLOAT => Value::Float(f32::from_bits(v.data)),
                    _ => return Err("invalid intent extra type".into()),
                };
                let entries = extras.get_or_insert_with(Vec::new);
                entries.retain(|(k, _)| k != &name);
                entries.push((name, value));
            }
            _ => {}
        }
    }
    if !categories.is_empty() {
        intent.categories = Some(array_order(categories, |s| s.as_str()));
    }
    Ok((
        FullIntent {
            intent,
            source_bounds: None,
            extras: None,
        },
        extras.map(|v| array_order(v, |(k, _)| k.as_str())),
    ))
}

impl Manifest {
    /// Accumulate one launcher activity's metadata. The limit is per file;
    /// reaching it stops the remainder of that file, including share targets.
    pub fn read(
        &mut self,
        e: &ManifestElement,
        ctx: &Ctx<'_>,
        activity: &ComponentName,
        max: usize,
        now: i64,
    ) -> Result<(), String> {
        let mut rank = 0;
        if e.name != "shortcuts" {
            eprintln!(
                "app host: shortcut: Invalid root '{}' in shortcut XML",
                e.name
            );
        }
        for c in &e.children {
            let a = ctx.obtain(c);
            match c.name.as_str() {
                "shortcut" => {
                    let Some(id) = nonempty(a.non_resource_string("shortcutId")) else {
                        continue;
                    };
                    let title = a.resource_id("shortcutShortLabel", 0);
                    if title == 0 || self.shortcuts.iter().any(|s| s.id == id) {
                        continue;
                    }
                    let enabled = a.boolean("enabled", true);
                    let mut s = Shortcut::parse(&Element {
                        name: "shortcut".into(),
                        attrs: vec![("id".into(), Xml::String(id))],
                        content: Vec::new(),
                    })?;
                    s.element = None;
                    s.activity = Some(activity.clone());
                    s.title_res_id = title;
                    s.text_res_id = a.resource_id("shortcutLongLabel", 0);
                    s.disabled_message_res_id = a.resource_id("shortcutDisabledMessage", 0);
                    s.icon_res_id = a.resource_id("icon", 0);
                    let theme = a.resource_id("splashScreenTheme", 0) as u32;
                    if theme != 0 {
                        s.splash_theme = Some(
                            ctx.res
                                .resource_name(theme)
                                .ok_or("invalid splash screen theme")?,
                        );
                    }
                    s.timestamp = now;
                    s.rank = rank;
                    s.flags = FLAG_IMMUTABLE
                        | if enabled {
                            FLAG_MANIFEST
                        } else {
                            FLAG_DISABLED
                        };
                    if s.icon_res_id != 0 {
                        s.flags |= FLAG_HAS_ICON_RES;
                    }
                    s.disabled_reason = if enabled { 0 } else { DISABLED_REASON_BY_APP };
                    let mut categories = Vec::new();
                    for child in &c.children {
                        match child.name.as_str() {
                            "intent" if enabled => {
                                let parsed = intent(child, ctx)?;
                                let invalid =
                                    parsed.0.intent.action.as_deref().is_none_or(str::is_empty);
                                s.intents.push(parsed);
                                if invalid {
                                    break;
                                }
                            }
                            "categories" => {
                                if let Some(name) =
                                    nonempty(ctx.obtain(child).non_resource_string("name"))
                                    && !categories.contains(&name)
                                {
                                    categories.push(name);
                                }
                            }
                            // The pinned parser has no capability-binding grammar.
                            other => eprintln!(
                                "app host: shortcut: Invalid tag '{other}' in shortcut XML"
                            ),
                        }
                    }
                    if !enabled {
                        s.intents = vec![(
                            FullIntent {
                                intent: Intent {
                                    action: Some("android.intent.action.VIEW".into()),
                                    ..Default::default()
                                },
                                source_bounds: None,
                                extras: None,
                            },
                            None,
                        )];
                    }
                    if s.intents.is_empty()
                        || s.intents.iter().any(|(i, x)| {
                            i.intent.action.as_deref().is_none_or(str::is_empty)
                                || x.as_ref().is_some_and(|x| {
                                    x.iter().any(|(_, v)| matches!(v, Value::Float(_)))
                                })
                        })
                    {
                        continue;
                    }
                    if rank as usize >= max {
                        return Ok(());
                    }
                    s.intents[0].0.intent.flags |= 0x1000_c000;
                    if !categories.is_empty() {
                        s.categories = Some(array_order(categories, |s| s.as_str()));
                    }
                    self.shortcuts.push(s);
                    rank += 1;
                }
                "share-target" => {
                    let Some(class) = nonempty(a.string("targetClass")) else {
                        continue;
                    };
                    let mut target = Element {
                        name: "share-target".into(),
                        attrs: Vec::new(),
                        content: Vec::new(),
                    };
                    attr(&mut target.attrs, "targetClass", Some(&class));
                    let (mut data, mut categories) = (Vec::new(), Vec::new());
                    for child in &c.children {
                        let a = ctx.obtain(child);
                        match child.name.as_str() {
                            "data" if a.kind("mimeType") == TYPE_STRING => {
                                let mut e = Element {
                                    name: "data".into(),
                                    attrs: Vec::new(),
                                    content: Vec::new(),
                                };
                                for name in [
                                    "scheme",
                                    "host",
                                    "port",
                                    "path",
                                    "pathPattern",
                                    "pathPrefix",
                                    "mimeType",
                                ] {
                                    attr(&mut e.attrs, name, a.string(name).as_deref());
                                }
                                data.push(Node::Element(e));
                            }
                            "category" if a.kind("name") == TYPE_STRING => {
                                if let Some(name) = nonempty(a.string("name"))
                                    && !categories.contains(&name)
                                {
                                    categories.push(name);
                                }
                            }
                            _ => {}
                        }
                    }
                    if data.is_empty() || categories.is_empty() {
                        continue;
                    }
                    target.content = data;
                    for name in array_order(categories, |s| s.as_str()) {
                        target.content.push(Node::Element(Element {
                            name: "category".into(),
                            attrs: vec![("name".into(), Xml::String(name))],
                            content: Vec::new(),
                        }));
                    }
                    self.share_targets.push(target);
                }
                other => eprintln!("app host: shortcut: Invalid tag '{other}' in shortcut XML"),
            }
        }
        Ok(())
    }
}

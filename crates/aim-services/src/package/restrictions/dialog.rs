//! `SuspendDialogInfo`'s XML owner, Android 16.0.0_r1.
use aim_android_xml::{Element, Value};

/// Resource IDs take precedence over literal text on restore.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DialogInfo {
    pub icon: i32,
    pub title_resource: i32,
    pub title: Option<String>,
    pub message_resource: i32,
    pub message: Option<String>,
    pub button_resource: i32,
    pub button: Option<String>,
    pub button_action: i32,
}

impl DialogInfo {
    pub fn restore(e: &Element) -> Self {
        // TypedXmlPullParser's default getters use the default for malformed
        // attributes too. The builder catches validation errors, retaining
        // the fields it assigned before the error.
        let int = |name| e.int(name).ok().flatten().unwrap_or(0);
        let mut out = Self::default();
        let icon = int("iconResId");
        if icon != 0 {
            if !valid_resource(icon) {
                return out;
            }
            out.icon = icon;
        }
        if !restore_text(
            e,
            "titleResId",
            "title",
            &mut out.title_resource,
            &mut out.title,
        ) {
            return out;
        }
        if !restore_text(
            e,
            "buttonTextResId",
            "buttonText",
            &mut out.button_resource,
            &mut out.button,
        ) {
            return out;
        }
        if !restore_text(
            e,
            "dialogMessageResId",
            "dialogMessage",
            &mut out.message_resource,
            &mut out.message,
        ) {
            return out;
        }
        let action = int("buttonAction");
        if matches!(action, 0 | 1) {
            out.button_action = action;
        }
        out
    }

    /// `saveToXml`: optional resource/string fields and the mandatory action.
    pub fn save(&self, name: &str) -> Element {
        let mut attrs = Vec::new();
        if self.icon != 0 {
            attrs.push(("iconResId".into(), Value::Int(self.icon)));
        }
        for (resource_name, text_name, resource, text) in [
            ("titleResId", "title", self.title_resource, &self.title),
            (
                "dialogMessageResId",
                "dialogMessage",
                self.message_resource,
                &self.message,
            ),
            (
                "buttonTextResId",
                "buttonText",
                self.button_resource,
                &self.button,
            ),
        ] {
            if resource != 0 {
                attrs.push((resource_name.into(), Value::Int(resource)));
            } else if let Some(text) = text {
                attrs.push((text_name.into(), Value::String(text.clone())));
            }
        }
        attrs.push(("buttonAction".into(), Value::Int(self.button_action)));
        Element {
            name: name.into(),
            attrs,
            content: Vec::new(),
        }
    }
}

fn restore_text(
    e: &Element,
    resource_name: &str,
    text_name: &str,
    resource: &mut i32,
    text: &mut Option<String>,
) -> bool {
    let id = e.int(resource_name).ok().flatten().unwrap_or(0);
    if id != 0 {
        if !valid_resource(id) {
            return false;
        }
        *resource = id;
    } else if let Some(value) = e.string(text_name) {
        if value.is_empty() {
            return false;
        }
        *text = Some(value.into_owned());
    }
    true
}

/// `ResourceId.isValid`, including IDs above 0x7f represented as negative ints.
fn valid_resource(id: i32) -> bool {
    id != -1 && (id & 0xff000000u32 as i32) != 0 && (id & 0x00ff0000) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_retains_only_prior_builder_fields() {
        let e = aim_android_xml::read(b"<dialog-info iconResId='-2130640895' title='kept' buttonText='' dialogMessage='lost' buttonAction='1'/>").unwrap();
        let d = DialogInfo::restore(&e);
        assert_eq!(d.icon, -2130640895);
        assert_eq!(d.title.as_deref(), Some("kept"));
        assert_eq!(d.button, None);
        assert_eq!(d.message, None);
        assert_eq!(d.button_action, 0);
    }

    #[test]
    fn malformed_attributes_default_and_resources_override_text() {
        let e = aim_android_xml::read(b"<dialog-info iconResId='bad' titleResId='2130771969' title='ignored' buttonText='ok' dialogMessage='message' buttonAction='bad'/>").unwrap();
        let d = DialogInfo::restore(&e);
        assert_eq!(d.icon, 0);
        assert_eq!(d.title, None);
        assert_eq!(d.title_resource, 2130771969);
        assert_eq!(d.button.as_deref(), Some("ok"));
        assert_eq!(d.message.as_deref(), Some("message"));
        assert_eq!(DialogInfo::restore(&d.save("dialog-info")), d);
    }
}

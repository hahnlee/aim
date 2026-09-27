//! `on` sections (system/core/init/action_parser.cpp, action.cpp).

use std::collections::BTreeMap;

use super::builtins::CommandSpec;
use super::expand::ANDROID_API_R;

/// One `on <triggers>` block.
///
/// An action fires either on its event trigger (`on boot && property:a=b`
/// fires when `boot` is triggered and `a` is `b` at that moment), or, when it
/// has no event trigger, whenever one of its properties changes to a
/// matching value while the others match too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// Empty when the action has only property triggers.
    pub event_trigger: String,
    /// `property:<name>=<value>`; the value `*` matches any non-empty value.
    pub property_triggers: BTreeMap<String, String>,
    pub commands: Vec<CommandSpec>,
    /// Guest path of the defining file (`<Builtin Action>` for init's own).
    pub filename: String,
    pub line: usize,
    /// Commands run in the vendor_init subcontext (vendor/odm scripts).
    pub vendor_subcontext: bool,
}

impl Action {
    /// `Action::BuildTriggersString`: property triggers in name order, then
    /// the event trigger, joined with ` && `.
    pub fn triggers_string(&self) -> String {
        let mut triggers: Vec<String> = self
            .property_triggers
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        if !self.event_trigger.is_empty() {
            triggers.push(self.event_trigger.clone());
        }
        triggers.join(" && ")
    }
}

/// `ParseTriggers`: triggers alternate with `&&`; at most one event trigger;
/// each property at most once.
pub(crate) fn parse_triggers(
    args: &[String],
    vendor_api_level: u32,
) -> Result<(String, BTreeMap<String, String>), String> {
    const PROP_PREFIX: &str = "property:";
    let mut event_trigger = String::new();
    let mut property_triggers = BTreeMap::new();
    for (index, arg) in args.iter().enumerate() {
        if arg.is_empty() {
            return Err("empty trigger is not valid".to_string());
        }
        if index % 2 == 1 {
            if arg != "&&" {
                return Err("&& is the only symbol allowed to concatenate actions".to_string());
            }
            continue;
        }
        if let Some(property) = arg.strip_prefix(PROP_PREFIX) {
            let Some(equal) = property.find('=') else {
                return Err("property trigger found without matching '='".to_string());
            };
            let name = property[..equal].to_string();
            let value = property[equal + 1..].to_string();
            // IsActionableProperty: only restricts vendor scripts when
            // ro.actionable_compatible_property.enabled is set, and then by
            // SELinux readability, which is permissive here (ADR 0012).
            if property_triggers.contains_key(&name) {
                return Err("multiple property triggers found for same property".to_string());
            }
            property_triggers.insert(name, value);
        } else {
            if !event_trigger.is_empty() {
                return Err("multiple event triggers are not allowed".to_string());
            }
            if vendor_api_level >= ANDROID_API_R
                && let Some(bad) = arg
                    .chars()
                    .find(|c| *c != '_' && *c != '-' && !c.is_ascii_alphanumeric())
            {
                return Err(format!("Illegal character '{bad}' in '{arg}'"));
            }
            event_trigger = arg.clone();
        }
    }
    Ok((event_trigger, property_triggers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_trigger_lists() {
        let (event, props) =
            parse_triggers(&strings(&["boot", "&&", "property:a.b=1"]), 36).unwrap();
        assert_eq!(event, "boot");
        assert_eq!(props.get("a.b").map(String::as_str), Some("1"));

        let (event, props) = parse_triggers(&strings(&["property:x=*"]), 36).unwrap();
        assert!(event.is_empty());
        assert_eq!(props["x"], "*");

        assert!(parse_triggers(&strings(&["boot", "and", "init"]), 36).is_err());
        assert!(parse_triggers(&strings(&["boot", "&&", "init"]), 36).is_err());
        assert!(parse_triggers(&strings(&["property:x"]), 36).is_err());
        assert!(parse_triggers(&strings(&["property:x=1", "&&", "property:x=2"]), 36).is_err());
        assert!(parse_triggers(&strings(&["bad.trigger"]), 36).is_err());
        assert!(parse_triggers(&strings(&["bad.trigger"]), 29).is_ok());
    }
}

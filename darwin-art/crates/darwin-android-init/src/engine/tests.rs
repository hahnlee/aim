use std::collections::BTreeMap;

use super::*;
use crate::rc::{IdResolver, InitScripts, ParseEnv, Parser, SectionKind};

const SCRIPT: &str = r#"
on early-init
    setprop a.early 1

on init
    setprop a.init ${ro.hardware:-generic}

on late-init
    trigger fs
    trigger boot

on fs
    setprop a.fs 1
    wait_for_prop a.ready 1

on boot
    setprop sys.x 1
    start foo

on property:a.early=1
    setprop b.from_prop 1

on property:sys.x=1 && property:a.fs=1
    setprop c 1

on property:c=1
    setprop d 1

on boot && property:a.init=generic
    setprop e 1

service foo /system/bin/foo
    user root
"#;

fn scripts() -> InitScripts {
    let ids = IdResolver::builtin();
    let props = BTreeMap::new();
    let env = ParseEnv {
        properties: &props,
        vendor_api_level: 36,
        ids: &ids,
    };
    let mut parser = Parser::new(env, SectionKind::Boot);
    parser.parse_data("/system/etc/init/hw/init.rc", SCRIPT.as_bytes());
    InitScripts {
        boot: parser.finish(),
        apex: Default::default(),
    }
}

#[test]
fn boot_queue_matches_second_stage_main() {
    let mut props: BTreeMap<String, String> = BTreeMap::new();
    props.insert(COLD_BOOT_DONE_PROP.to_string(), "true".to_string());
    let mut manager = ActionManager::new(scripts(), 36);
    manager.queue_boot(&props);
    let mut executor = DryRunExecutor::default();

    let (first, step) = manager.run_until_blocked(&mut props, &mut executor, 1000);
    assert_eq!(
        step,
        Step::WaitingForProperty {
            name: "a.ready".to_string(),
            value: "1".to_string()
        }
    );
    // The property service reports the change; the waiter releases.
    props.insert("a.ready".to_string(), "1".to_string());
    manager.property_changed("a.ready", "1");
    let (rest, step) = manager.run_until_blocked(&mut props, &mut executor, 1000);
    assert_eq!(step, Step::Idle);

    let order: Vec<String> = first
        .iter()
        .chain(rest.iter())
        .map(|c| format!("{} | {}", c.action, c.command))
        .collect();
    assert_eq!(
        order,
        vec![
            "SetupCgroups | SetupCgroups",
            "SetKptrRestrict | SetKptrRestrict",
            "TestPerfEventSelinux | TestPerfEventSelinux",
            "early-init | setprop a.early 1",
            "ConnectEarlyStageSnapuserd | ConnectEarlyStageSnapuserd",
            "wait_for_coldboot_done | wait_for_coldboot_done",
            "CheckTradeInModeStatus | CheckTradeInModeStatus",
            "SetMmapRndBits | SetMmapRndBits",
            "KeychordInit | KeychordInit",
            "init | setprop a.init ${ro.hardware:-generic}",
            "late-init | trigger fs",
            "late-init | trigger boot",
            "queue_property_triggers | queue_property_triggers",
            "fs | setprop a.fs 1",
            "fs | wait_for_prop a.ready 1",
            "boot | setprop sys.x 1",
            "boot | start foo",
            "a.init=generic && boot | setprop e 1",
            "enable_property_trigger | enable_property_trigger",
            "a.early=1 | setprop b.from_prop 1",
            "a.fs=1 && sys.x=1 | setprop c 1",
            "c=1 | setprop d 1",
        ]
    );
    assert_eq!(props["a.init"], "generic");
    assert_eq!(props["d"], "1");
    assert!(manager.property_triggers_enabled());
    // Builtin actions are oneshot and removed after running.
    assert_eq!(manager.actions().count(), 9);
    assert!(executor.commands.contains(&Command::Start {
        service: "foo".to_string()
    }));
}

#[test]
fn charger_mode_and_star_triggers() {
    let mut props: BTreeMap<String, String> = BTreeMap::new();
    props.insert(COLD_BOOT_DONE_PROP.to_string(), "true".to_string());
    props.insert("ro.bootmode".to_string(), "charger".to_string());
    let mut manager = ActionManager::new(scripts(), 36);
    manager.queue_boot(&props);
    let mut executor = DryRunExecutor::default();
    let (ran, _) = manager.run_until_blocked(&mut props, &mut executor, 1000);
    assert!(!ran.iter().any(|c| c.action == "late-init"));
}

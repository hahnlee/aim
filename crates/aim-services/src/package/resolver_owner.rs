//! PMS setUpCustomResolverActivity/setPlatformPackage, android-16.0.0_r1.
//! Copyright AOSP, Apache-2.0. Registration order belongs to the native scan.
use super::{model::{State,PackageUserState},info::{ActivityInfo,ComponentInfo,PackageItemInfo,Target,generate_application_info},preferred};
use std::sync::Arc;
const LAUNCH_MULTIPLE: i32 = 0;
const DOCUMENT_LAUNCH_NEVER: i32 = 3;
const FLAG_EXCLUDE_FROM_RECENTS: i32 = 0x20;
const FLAG_HARDWARE_ACCELERATED: i32 = 0x200;
const FLAG_RELINQUISH_TASK_IDENTITY: i32 = 0x1000;
const FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES: i32 = 0x10000;
const RESIZE_MODE_RESIZEABLE: i32 = 2;
const SCREEN_ORIENTATION_UNSPECIFIED: i32 = -1;
const CONFIG_KEYBOARD: i32 = 0x10;
const CONFIG_KEYBOARD_HIDDEN: i32 = 0x20;
const CONFIG_ORIENTATION: i32 = 0x80;
const CONFIG_SCREEN_LAYOUT: i32 = 0x100;
const CONFIG_SCREEN_SIZE: i32 = 0x400;
const CONFIG_SMALLEST_SCREEN_SIZE: i32 = 0x800;

#[derive(Clone,Debug,PartialEq)]
pub struct Owner {pub activity:ActivityInfo,pub replaced:bool, pub events_seen:usize}
impl Owner {
    pub fn capture(state:&State,committed_packages:&[String],previous:Option<&Self>)->Result<Self,String> {
        let custom=state.platform.custom_resolver.as_deref().and_then(preferred::unflatten);
        let mut activity=previous.map(|owner|owner.activity.clone());let mut replaced=previous.is_some_and(|owner|owner.replaced);let mut platform=previous.is_some();
        let seen=previous.map_or(0,|owner|owner.events_seen);
        if seen>committed_packages.len() {return Err("native resolver registration history moved backwards".into());}
        for name in &committed_packages[seen..] {
            if name!="android" && !custom.as_ref().is_some_and(|custom|custom.package==*name) {continue;}
            let setting=state.packages.get(name).ok_or("native resolver committed setting unavailable")?;
            let pkg=setting.pkg.as_deref().ok_or("native resolver committed code unavailable")?;
            let target=Target {sys:&state.system,pkg,ps:setting,state:&PackageUserState::default(),user:0};
            if custom.as_ref().is_some_and(|custom|custom.package==pkg.package_name) {
                let mut app=generate_application_info(&target,0).ok_or("native custom resolver application unavailable")?;
                apply_overlays(&mut app,setting);
                let mut selected=match activity.take() {Some(activity)=>activity,None=>blank(app.clone())};
                selected.info.application_info=Arc::new(app);selected.info.item.name=Some(custom.as_ref().unwrap().class.clone());
                selected.info.item.package_name=Some(pkg.package_name.clone());selected.info.process_name=pkg.process_name.clone();
                selected.launch_mode=0;selected.flags=0x20|0x100|0x10000|0x200;selected.theme=0;
                selected.info.exported=true;selected.info.enabled=true;activity=Some(selected);replaced=true;
            }
            if pkg.package_name=="android" {
                platform=true;
                if !replaced {
                    let mut app=generate_application_info(&target,0).ok_or("native platform application unavailable")?;
                    apply_overlays(&mut app,setting);
                    activity=Some(platform_activity(app,state.platform.resolver_theme));
                }
            }
        }
        if !platform {return Err("native resolver platform registration incomplete".into());}
        let mut activity=activity.ok_or("native resolver activity unavailable")?;
        if let Some(setting)=activity.info.item.package_name.as_ref().and_then(|name|state.packages.get(name)) {
            let mut app=(*activity.info.application_info).clone();apply_overlays(&mut app,setting);activity.info.application_info=Arc::new(app);
        }
        Ok(Self {activity,replaced,events_seen:committed_packages.len()})
    }
}
fn apply_overlays(app:&mut super::info::ApplicationInfo,setting:&super::model::PackageState) {
    if let Some(paths)=setting.users.get(&0).and_then(|user|user.overlay_paths.as_ref()) {
        app.overlay_paths=Some(paths.overlay_paths.clone());app.resource_dirs=Some(paths.resource_dirs.clone());
    } else {app.overlay_paths=None;app.resource_dirs=None;}
}
fn platform_activity(app:super::info::ApplicationInfo,theme:i32)->ActivityInfo {
    let package_name=app.item.package_name.clone();
    ActivityInfo {
            info: ComponentInfo {
                item: PackageItemInfo {
                    name: Some("com.android.internal.app.ResolverActivity".into()),
                    package_name,
                    ..PackageItemInfo::default()
                },
                application_info: Arc::new(app),
                process_name: Some("system:ui".into()),
                split_name: None,
                attribution_tags: None,
                description_res: 0,
                enabled: true,
                exported: true,
                direct_boot_aware: false,
            },
            theme: theme,
            launch_mode: LAUNCH_MULTIPLE,
            document_launch_mode: DOCUMENT_LAUNCH_NEVER,
            permission: None,
            task_affinity: None,
            target_activity: None,
            flags: FLAG_EXCLUDE_FROM_RECENTS
                | FLAG_RELINQUISH_TASK_IDENTITY
                | FLAG_CAN_DISPLAY_ON_REMOTE_DEVICES
                | FLAG_HARDWARE_ACCELERATED,
            private_flags: 0,
            screen_orientation: SCREEN_ORIENTATION_UNSPECIFIED,
            config_changes: CONFIG_SCREEN_SIZE
                | CONFIG_SMALLEST_SCREEN_SIZE
                | CONFIG_SCREEN_LAYOUT
                | CONFIG_ORIENTATION
                | CONFIG_KEYBOARD
                | CONFIG_KEYBOARD_HIDDEN,
            soft_input_mode: 0,
            ui_options: 0,
            parent_activity_name: None,
            persistable_mode: 0,
            max_recents: 0,
            lock_task_launch_mode: 0,
            window_layout: None,
            resize_mode: RESIZE_MODE_RESIZEABLE,
            requested_vr_component: None,
            rotation_animation: -1,
            color_mode: 0,
            max_aspect_ratio: 0.0,
            min_aspect_ratio: 0.0,
            supports_size_changes: false,
            known_activity_embedding_certs: None,
            required_display_category: None,
            require_content_uri_permission_from_caller: 0,
        }
}
fn blank(app:super::info::ApplicationInfo)->ActivityInfo {
    let mut value=platform_activity(app,0);
    value.document_launch_mode=0;value.config_changes=0;value.flags=0;value.info.item.name=None;value.info.item.package_name=None;
    value.info.process_name=None;value.info.enabled=true;value.info.exported=false;value
}

//! System status icons (`IStatusBar.setIcon`, `removeIcon`, from the
//! native status bar, `shell.rs`) as menu bar items of the package that
//! set them (`status.rs`).

use std::cell::RefCell;
use std::collections::HashMap;

use aim_host_display::shell::StatusIcon;

use crate::objc::{Id, nsstring};

thread_local! {
    /// The items (retained), by package and slot.
    static ICONS: RefCell<HashMap<(String, String), usize>> = RefCell::new(HashMap::new());
}

/// Show, update or remove `package`'s system status icon in `slot`
/// (`IStatusBar.setIcon`, `removeIcon`): its image as a template, with
/// its number if it has one; clicked, it brings the app to the front.
pub fn system_icon(package: &str, slot: &str, icon: Option<&StatusIcon>) {
    let key = (package.to_string(), slot.to_string());
    let Some(icon) = icon.filter(|i| i.visible) else {
        if let Some(item) = ICONS.with_borrow_mut(|items| items.remove(&key)) {
            crate::status::remove_item(item as Id);
        }
        return;
    };
    let item = match ICONS.with_borrow(|items| items.get(&key).copied()) {
        Some(item) => item as Id,
        None => {
            let item = crate::status::launch_item();
            ICONS.with_borrow_mut(|items| items.insert(key, item as usize));
            item
        }
    };
    let button = send!(item, c"button" => Id);
    send!(button, c"setImage:" => (), Id = crate::status::icon(icon.image.as_ref()));
    let number = if icon.number > 0 {
        icon.number.to_string()
    } else {
        String::new()
    };
    send!(button, c"setTitle:" => (), Id = nsstring(&number));
    send!(button, c"setAccessibilityLabel:" => (), Id = nsstring(&icon.description));
}

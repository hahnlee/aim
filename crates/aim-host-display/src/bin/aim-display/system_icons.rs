//! System status icons (`IStatusBar.setIcon`, `removeIcon`, from the
//! native status bar, `shell.rs`) as menu bar items of the package that
//! set them (`status.rs`), and the pinned task's item (screen pinning).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use aim_host_display::shell::StatusIcon;

use crate::objc::{Id, class, nsstring};

thread_local! {
    /// The items (retained), by package and slot.
    static ICONS: RefCell<HashMap<(String, String), usize>> = RefCell::new(HashMap::new());
    /// The pinned task's item (retained).
    static PINNED: Cell<Option<usize>> = const { Cell::new(None) };
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

/// While task `task` is pinned (only one task can be), a menu bar item
/// with a pin says so; its menu unpins the task, as holding Back and
/// Overview does on a phone.
pub fn pinned(task: i32, pinned: bool) {
    if let Some(item) = PINNED.take() {
        crate::status::remove_item(item as Id);
    }
    if !pinned {
        return;
    }
    let item = crate::status::launch_item();
    let image = send!(class(c"NSImage"), c"imageWithSystemSymbolName:accessibilityDescription:" => Id,
        Id = nsstring("pin.fill"), Id = nsstring("Pinned"));
    let button = send!(item, c"button" => Id);
    send!(button, c"setImage:" => (), Id = image);
    let menu = send!(class(c"NSMenu"), c"new" => Id);
    let unpin = crate::status::menu_item("Unpin", &task.to_string(), crate::status::UNPIN);
    send!(menu, c"addItem:" => (), Id = unpin);
    send!(item, c"setMenu:" => (), Id = menu);
    crate::objc::release(menu);
    PINNED.set(Some(item as usize));
}

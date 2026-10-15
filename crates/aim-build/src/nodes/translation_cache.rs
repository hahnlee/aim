//! `translation-cache`: the x18/TPIDR_EL0 rewrites of every ELF file of the
//! derived image, made ahead of a boot, into the derived image's own
//! `translated/` (`linux-translate --image`, docs/storage.md). The system
//! image already holds the original's; what is left are the overlay's
//! files, and every file after a translator version change. They go
//! through the derived image's shadow file, attached writable for this.

use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use aim_storage::system;
use std::process::Command;

pub fn node() -> Node {
    Node {
        name: "translation-cache".into(),
        deps: vec![Dep::on("derived-image"), Dep::on("host/linux-translate")],
        inputs: Vec::new(),
        outputs: Vec::new(),
        tools: Vec::new(),
        recipe: 2,
        action: Action::TranslationCache,
        boot: true,
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    let (image, shadow) = (aim_paths::system_image(), aim_paths::derived_image_shadow());
    let mount = aim_paths::derived_image_mount();
    let _lease = system::ImageLease::write(&image)?;
    system::attach(&image, Some(&shadow), &mount, true)?;
    let translated = log.run(
        Command::new(ctx.workspace.host_bin("linux-translate"))
            .arg("--image")
            .arg(&mount),
    );
    system::detach(&image, Some(&shadow))?;
    system::attach(&image, Some(&shadow), &mount, false)?;
    translated
}

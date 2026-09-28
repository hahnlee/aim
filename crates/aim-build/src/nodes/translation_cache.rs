//! `translation-cache`: the x18/TPIDR_EL0 rewrites of every ELF file of the
//! derived image, made ahead of a boot (`linux-translate`, into the cache
//! linux-run reads by default, `~/Library/Caches/aim/translated`).

use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use std::process::Command;

pub fn node() -> Node {
    Node {
        name: "translation-cache".into(),
        deps: vec![Dep::on("derived-image"), Dep::on("host/linux-translate")],
        inputs: Vec::new(),
        outputs: Vec::new(),
        tools: Vec::new(),
        recipe: 1,
        action: Action::TranslationCache,
        boot: true,
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    log.run(Command::new(ctx.workspace.host_bin("linux-translate")).arg(aim_paths::derived_image()))
}

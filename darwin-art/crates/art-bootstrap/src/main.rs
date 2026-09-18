//! Thin CLI entrypoint for the ART bootstrap command graph.
//!
//! The implementation lives in `commands.rs` so changing the launcher or
//! help text does not make the native build command module part of the entry
//! point's source boundary. Native compilation itself is further isolated in
//! `native_build.rs` and cached per translation unit.

mod build_context;
mod commands;
mod help;
mod native_build;
mod native_cache;
mod native_graph;
mod native_link_recipe;
mod support;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    pub(crate) fn getpagesize() -> i32;
}

fn main() {
    if let Err(error) = commands::run() {
        eprintln!("art-bootstrap: {error}");
        std::process::exit(1);
    }
}

use super::*;

/// Export the original NDK graphics modules, not a hand-picked API subset.
/// These functions are found by Android library lookup rather than static
/// relocations, so the final shared boundary must retain them explicitly.
pub(crate) fn bitmap_exports(root: &Path) -> Result<Vec<String>> {
    let mut exports = Vec::new();
    for (module, prefix, count) in [
        ("ndk_bitmap", "_AndroidBitmap_", 6),
        ("ndk_imagedecoder", "_AImageDecoder", 33),
    ] {
        let object = root.join(format!("_build/android-graphics-jni/objects/{module}.o"));
        let symbols = command_output(Command::new("nm").args(["-gU"]).arg(object))?;
        let module_exports: Vec<String> = symbols
            .lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                match fields.as_slice() {
                    [_, "T", name] if name.starts_with(prefix) => Some((*name).to_owned()),
                    _ => None,
                }
            })
            .collect();
        if module_exports.len() != count {
            return Err(format!(
                "original NDK {module} export count={}, expected={count}",
                module_exports.len()
            )
            .into());
        }
        exports.extend(module_exports);
    }
    Ok(exports)
}

use super::*;

pub(crate) fn compile_cached_probe_tu(
    command: &mut Command,
    object: &Path,
    cache_path: &Path,
    compiler_identity: &str,
) -> Result<bool> {
    let mut cache = FileHashCache::load(cache_path)?;
    let compiled = compile_with_dependency_cache(command, object, compiler_identity, &mut cache)?;
    cache.save(cache_path)?;
    Ok(compiled)
}

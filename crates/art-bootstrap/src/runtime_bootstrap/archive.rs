use super::*;
use aim_build_contract::RuntimeFlavor;

pub(crate) fn finalize(
    staged: &RuntimeBootstrapStaging,
    flavor: RuntimeFlavor,
    compiled: RuntimeBootstrapCompiled,
) -> Result<()> {
    let archive = staged.build_dir.join(flavor.archive_name());
    // Runtime TUs are shared between headless and graphics flavors. A rebuild
    // performed by one flavor can therefore refresh a common object while the
    // other flavor reports every input as cached. Reuse that flavor's archive
    // only when it is at least as new as every object it would contain.
    let archive_mtime = fs::metadata(&archive).and_then(|meta| meta.modified()).ok();
    let archive_is_current = archive_mtime.is_some_and(|archive_mtime| {
        compiled.objects.iter().all(|object| {
            fs::metadata(object)
                .and_then(|meta| meta.modified())
                .is_ok_and(|object_mtime| object_mtime <= archive_mtime)
        })
    });
    let reused_archive = compiled.compiled_objects == 0 && archive_is_current;
    if !reused_archive {
        create_archive(&archive, &compiled.objects)?;
    }
    println!(
        "{}: ART runtime initialization spine Mach-O objects={} compiled={} cached={} archive={}{}",
        if flavor.real_graphics() {
            "build-runtime-graphics-bootstrap"
        } else {
            "build-runtime-bootstrap"
        },
        compiled.objects.len(),
        compiled.compiled_objects,
        compiled.cached_objects,
        archive.display(),
        if reused_archive { " reused" } else { "" }
    );
    Ok(())
}

use super::adapter_jobs::adapter_jobs;
use super::runtime_jobs::runtime_jobs;
use super::seed_jobs::bootstrap_jobs;
use super::*;
use crate::native_build::compile_pending_native;
use aim_build_contract::{RUNTIME_CACHE_IDENTITY, RuntimeFlavor};

pub(crate) struct RuntimeBootstrapCompiled {
    pub(crate) objects: Vec<PathBuf>,
    pub(crate) compiled_objects: usize,
    pub(crate) cached_objects: usize,
}

pub(crate) fn compile(
    staged: &RuntimeBootstrapStaging,
    flavor: RuntimeFlavor,
) -> Result<RuntimeBootstrapCompiled> {
    let real_graphics = flavor.real_graphics();
    let includes = staged
        .includes
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let runtime_includes = staged
        .runtime_includes
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    let compiler_identity = format!(
        "{}macOS {} ({})\nruntime-cache={RUNTIME_CACHE_IDENTITY}",
        command_output(Command::new("clang++").arg("--version"))?,
        command_output(Command::new("sw_vers").arg("-productVersion"))?.trim(),
        command_output(Command::new("sw_vers").arg("-buildVersion"))?.trim()
    );
    let (mut objects, bootstrap_compiled, bootstrap_cached) = compile_pending_native(
        bootstrap_jobs(staged, real_graphics, &includes, &runtime_includes),
        &compiler_identity,
    )?;
    let mut compiled_objects = bootstrap_compiled;
    let mut cached_objects = bootstrap_cached;

    let adapter_jobs = adapter_jobs(staged, real_graphics, &includes, &runtime_includes);
    let (adapter_objects, adapter_compiled, adapter_cached) =
        compile_pending_native(adapter_jobs, &compiler_identity)?;
    compiled_objects += adapter_compiled;
    cached_objects += adapter_cached;
    objects.extend(adapter_objects);

    let runtime_jobs = runtime_jobs(staged, &runtime_includes);
    let (runtime_objects, runtime_compiled, runtime_cached) =
        compile_pending_native(runtime_jobs, &compiler_identity)?;
    compiled_objects += runtime_compiled;
    cached_objects += runtime_cached;
    for object in runtime_objects {
        let kind = command_output(Command::new("file").arg(&object))?;
        if !kind.contains("Mach-O 64-bit object arm64") {
            return Err(format!("unexpected Runtime object format: {kind}").into());
        }
        objects.push(object);
    }
    let runtime_object = staged.runtime_core_object_dir.join("runtime.cc.o");
    let symbols = command_output(Command::new("nm").args(["-gU"]).arg(&runtime_object))?;
    if !symbols.contains("_ZN3art7Runtime6Create") {
        return Err("compiled Runtime object does not export Runtime::Create".into());
    }
    Ok(RuntimeBootstrapCompiled {
        objects,
        compiled_objects,
        cached_objects,
    })
}

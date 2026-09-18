//! Input and content-stamp manifest for independently cached native probes.
//!
//! Probe invalidation is deliberately separate from the broad runtime graph
//! digest. This module owns the phase input closure and stable content stamps;
//! graph string emission consumes the resulting value-only manifest.

use std::io;
use std::path::{Path, PathBuf};

use super::inputs::{native_owner_content_stamp, probe_content_stamp, probe_inputs};

pub(crate) struct ProbeGraphInputs {
    pub(crate) network_probe_inputs: String,
    pub(crate) hwui_probe_inputs: String,
    pub(crate) graphics_probe_inputs: String,
    pub(crate) graphics_gpu_probe_inputs: String,
    pub(crate) graphics_phase_inputs: String,
    pub(crate) graphics_input_inputs: String,
    pub(crate) graphics_state_inputs: String,
    pub(crate) graphics_session_inputs: String,
    pub(crate) jni_acceptance_inputs: String,
    pub(crate) app_bootstrap_inputs: String,
    pub(crate) app_resources_inputs: String,
    pub(crate) app_activity_inputs: String,
    pub(crate) app_presentation_inputs: String,
    pub(crate) network_probe_stamp: PathBuf,
    pub(crate) hwui_probe_stamp: PathBuf,
    pub(crate) graphics_probe_stamp: PathBuf,
    pub(crate) graphics_gpu_probe_stamp: PathBuf,
    pub(crate) graphics_phase_stamp: PathBuf,
    pub(crate) graphics_input_stamp: PathBuf,
    pub(crate) graphics_state_stamp: PathBuf,
    pub(crate) graphics_session_stamp: PathBuf,
    pub(crate) jni_acceptance_stamp: PathBuf,
    pub(crate) app_bootstrap_stamp: PathBuf,
    pub(crate) app_resources_stamp: PathBuf,
    pub(crate) app_activity_stamp: PathBuf,
    pub(crate) app_presentation_stamp: PathBuf,
    pub(crate) runtime_entry_stamp: PathBuf,
}

pub(crate) fn collect(root: &Path) -> io::Result<ProbeGraphInputs> {
    let network_probe_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_network_probe.cc",
            "probes/runtime_network_probe.h",
        ],
    );
    let hwui_probe_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_hwui_probe.cc",
            "probes/runtime_hwui_probe.h",
            "tools/bionic-fs-facade/include/darwin_art_bionic_fs.h",
            "tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h",
            "compat/network/multinetwork.h",
        ],
    );
    let graphics_probe_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_graphics_probe.cc",
            "probes/runtime_graphics_probe.h",
            "runtime/art/native_registration.cc",
            "runtime/art/native_registration.h",
            "runtime/art/boot_native_registration.cc",
            "compat/art/boot_native_libraries.cc",
            "compat/art/boot_native_libraries.h",
            "runtime/art/vm_bootstrap.cc",
            "runtime/art/vm_bootstrap.h",
            "probes/runtime_registration_fixture.cc",
            "probes/runtime_registration_fixture.h",
            "probes/graphics_fixture_state.h",
            "compat/darwin_surface_bridge.h",
            "compat/input/darwin_hardware_key_translation.h",
            "compat/darwin_framework_natives.h",
            "compat/darwin_hwui_gpu_mode.h",
        ],
    );
    let graphics_gpu_probe_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_graphics_gpu.cc",
            "probes/runtime_graphics_gpu.h",
            "runtime/embedding/graphics_state.h",
            "probes/runtime_hwui_probe.h",
        ],
    );
    let graphics_phase_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_graphics_phase.cc",
            "probes/runtime_graphics_phase.h",
            "probes/runtime_graphics_probe.h",
            "probes/runtime_frame_probe.h",
        ],
    );
    let graphics_input_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_graphics_input.cc",
            "runtime/framework/input/event_ingress.cc",
            "runtime/framework/input/event_ingress.h",
            "runtime/framework/display/vsync_source.cc",
            "runtime/framework/display/vsync_source.h",
            "probes/runtime_graphics_probe.h",
            "probes/runtime_graphics_probe_internal.h",
            "runtime/art/process_state.h",
        ],
    );
    let graphics_state_inputs = probe_inputs(
        root,
        &[
            "runtime/embedding/graphics_state.cc",
            "runtime/embedding/graphics_state.h",
            "compat/darwin_surface_bridge.h",
            "compat/input/darwin_hardware_key_translation.h",
        ],
    );
    let graphics_session_inputs = probe_inputs(
        root,
        &[
            "runtime/embedding/graphics_session.cc",
            "runtime/embedding/graphics_session.h",
            "runtime/embedding/session_lifetime.h",
            "runtime/embedding/graphics_state.h",
            "runtime/art/process_state.h",
            "include/darwin_art/darwin_art.h",
        ],
    );
    let jni_acceptance_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_jni_acceptance_probe.cc",
            "probes/runtime_jni_acceptance_probe.h",
            "probes/runtime_abi_probe.h",
            "compat/jni/scoped_local_frame.h",
            "probes/runtime_upstream_test.h",
            "probes/runtime_jit_loop_checkpoint.h",
            "probes/runtime_jit_invoke_custom.h",
            "probes/runtime_jit_specialized_intrinsics.h",
            "probes/runtime_jit_string_intrinsics.h",
            "probes/runtime_jit_string_hidden_intrinsics.h",
            "probes/runtime_jit_system_arraycopy.h",
            "probes/runtime_jit_math_hinvoke.h",
            "probes/runtime_jit_crc32.h",
            "probes/runtime_jit_memory.h",
            "probes/runtime_jit_reference_boxing.h",
            "probes/runtime_jit_unsafe_intrinsics.h",
        ],
    );
    let app_bootstrap_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_app_bootstrap.cc",
            "probes/runtime_app_bootstrap.h",
            "runtime/art/process_state.h",
        ],
    );
    let app_presentation_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_app_presentation.cc",
            "probes/runtime_app_presentation.h",
            "probes/runtime_app_activity.h",
            "probes/runtime_app_resources.h",
            "probes/runtime_graphics_phase.h",
            "runtime/embedding/graphics_state.h",
        ],
    );
    let app_resources_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_app_resources.cc",
            "probes/runtime_app_resources.h",
        ],
    );
    let app_activity_inputs = probe_inputs(
        root,
        &[
            "probes/runtime_app_activity.cc",
            "runtime/framework/app/application_binding.h",
            "runtime/framework/app/process_attachment.h",
            "runtime/framework/pm/declared_providers.h",
            "probes/runtime_app_activity.h",
            "probes/runtime_app_resources.h",
        ],
    );
    // Ninja normally invalidates by mtime.  These stable content stamps make
    // the probe boundary robust when a checkout/materializer preserves source
    // mtimes (and keep each phase's content identity independent of the broad
    // runtime graph digest).  The audit path regenerates the graph before
    // asking Ninja for its warm/no-op result.
    let network_probe_stamp = probe_content_stamp(
        root,
        "network",
        &[
            "probes/runtime_network_probe.cc",
            "probes/runtime_network_probe.h",
        ],
    )?;
    let hwui_probe_stamp = probe_content_stamp(
        root,
        "hwui",
        &[
            "probes/runtime_hwui_probe.cc",
            "probes/runtime_hwui_probe.h",
            "tools/bionic-fs-facade/include/darwin_art_bionic_fs.h",
            "tools/bionic-socket-broker-adapter/include/darwin_art_bionic_socket_broker.h",
            "compat/network/multinetwork.h",
        ],
    )?;
    let graphics_probe_stamp = probe_content_stamp(
        root,
        "graphics",
        &[
            "probes/runtime_graphics_probe.cc",
            "probes/runtime_graphics_probe.h",
            "compat/darwin_surface_bridge.h",
            "compat/input/darwin_hardware_key_translation.h",
            "compat/darwin_framework_natives.h",
            "compat/darwin_hwui_gpu_mode.h",
        ],
    )?;
    let graphics_gpu_probe_stamp = probe_content_stamp(
        root,
        "graphics-gpu",
        &[
            "probes/runtime_graphics_gpu.cc",
            "probes/runtime_graphics_gpu.h",
            "runtime/embedding/graphics_state.h",
            "probes/runtime_hwui_probe.h",
        ],
    )?;
    let graphics_phase_stamp = probe_content_stamp(
        root,
        "graphics-phase",
        &[
            "probes/runtime_graphics_phase.cc",
            "probes/runtime_graphics_phase.h",
            "probes/runtime_graphics_probe.h",
            "probes/runtime_frame_probe.h",
        ],
    )?;
    let graphics_input_stamp = probe_content_stamp(
        root,
        "graphics-input",
        &[
            "probes/runtime_graphics_input.cc",
            "runtime/framework/input/event_ingress.cc",
            "runtime/framework/input/event_ingress.h",
            "runtime/framework/display/vsync_source.cc",
            "runtime/framework/display/vsync_source.h",
            "probes/runtime_graphics_probe.h",
            "probes/runtime_graphics_probe_internal.h",
            "runtime/art/process_state.h",
        ],
    )?;
    let graphics_state_stamp = native_owner_content_stamp(
        root,
        "graphics-state",
        &[
            "runtime/embedding/graphics_state.cc",
            "runtime/embedding/graphics_state.h",
            "compat/darwin_surface_bridge.h",
            "compat/input/darwin_hardware_key_translation.h",
        ],
    )?;
    let graphics_session_stamp = native_owner_content_stamp(
        root,
        "graphics-session",
        &[
            "runtime/embedding/graphics_session.cc",
            "runtime/embedding/graphics_session.h",
            "runtime/embedding/session_lifetime.h",
            "runtime/embedding/graphics_state.h",
        ],
    )?;
    let jni_acceptance_stamp = probe_content_stamp(
        root,
        "jni-acceptance",
        &[
            "probes/runtime_jni_acceptance_probe.cc",
            "probes/runtime_jni_acceptance_probe.h",
            "probes/runtime_abi_probe.h",
            "compat/jni/scoped_local_frame.h",
            "probes/runtime_upstream_test.h",
            "probes/runtime_jit_invoke_custom.h",
            "probes/runtime_jit_specialized_intrinsics.h",
            "probes/runtime_jit_string_intrinsics.h",
            "probes/runtime_jit_string_hidden_intrinsics.h",
            "probes/runtime_jit_system_arraycopy.h",
            "probes/runtime_jit_math_hinvoke.h",
            "probes/runtime_jit_crc32.h",
            "probes/runtime_jit_memory.h",
            "probes/runtime_jit_reference_boxing.h",
            "probes/runtime_jit_unsafe_intrinsics.h",
        ],
    )?;
    let app_bootstrap_stamp = probe_content_stamp(
        root,
        "app-bootstrap",
        &[
            "probes/runtime_app_bootstrap.cc",
            "probes/runtime_app_bootstrap.h",
            "runtime/art/process_state.h",
        ],
    )?;
    let app_presentation_stamp = probe_content_stamp(
        root,
        "app-presentation",
        &[
            "probes/runtime_app_presentation.cc",
            "probes/runtime_app_presentation.h",
            "probes/runtime_app_activity.h",
            "probes/runtime_app_resources.h",
            "probes/runtime_graphics_phase.h",
            "runtime/embedding/graphics_state.h",
        ],
    )?;
    let app_resources_stamp = probe_content_stamp(
        root,
        "app-resources",
        &[
            "probes/runtime_app_resources.cc",
            "probes/runtime_app_resources.h",
        ],
    )?;
    let app_activity_stamp = probe_content_stamp(
        root,
        "app-activity",
        &[
            "probes/runtime_app_activity.cc",
            "runtime/framework/app/application_binding.h",
            "runtime/framework/app/process_attachment.h",
            "runtime/framework/pm/declared_providers.h",
            "probes/runtime_app_activity.h",
            "probes/runtime_app_resources.h",
        ],
    )?;
    // The graphics link command compiles the process entry TU as part of its
    // phase orchestration. Keep that source's content identity as an explicit
    // Ninja prerequisite so a checkout that preserves mtimes cannot reuse a
    // dylib linked against an older entry point.
    let runtime_entry_stamp = native_owner_content_stamp(
        root,
        "runtime-entry",
        &[
            "compat/binder/service_endpoint.h",
            "compat/process/service_endpoint.h",
            "runtime/framework/system/application_shared_memory.h",
            "runtime/framework/app/main_loop.h",
            "runtime/framework/app/process_entry.h",
            "runtime/framework/app/process_registration.h",
            "runtime/framework/app/process_entry.cc",
            "runtime/framework/app/process_registration.cc",
            "runtime/framework/am/application_binding.h",
            "runtime/framework/am/process_launch.h",
            "runtime/framework/am/connection_death_jni.h",
            "compat/binder/proxy_death_recipient.h",
            "runtime/framework/connectivity/network_path_abi.h",
            "runtime/framework/connectivity/network_provider_jni.h",
            "runtime/framework/power/power_state_platform.h",
            "runtime/framework/power/power_state_jni.h",
            "runtime/art/native_registration.cc",
            "runtime/art/native_registration.h",
            "runtime/art/process_state.h",
            "runtime/art/process_state.cc",
            "runtime/art/boot_native_registration.cc",
            "compat/art/boot_native_libraries.cc",
            "compat/art/boot_native_libraries.h",
            "runtime/art/vm_bootstrap.h",
            "runtime/art/vm_bootstrap.cc",
            "runtime/embedding/process_config.h",
            "runtime/embedding/process_config.cc",
            "runtime/embedding/process_entry.h",
            "runtime/embedding/process_entry.cc",
            "runtime/embedding/process_shutdown.h",
            "runtime/embedding/process_shutdown.cc",
            "runtime/art/vm_shutdown.h",
            "runtime/art/vm_shutdown.cc",
            "runtime/framework/app/process_shutdown.h",
            "runtime/framework/app/process_shutdown.cc",
            "runtime/embedding/graphics_session.h",
            "runtime/embedding/graphics_session.cc",
            "runtime/embedding/session_lifetime.h",
            "runtime/embedding/graphics_state.h",
            "runtime/embedding/graphics_state.cc",
            "runtime/framework/input/event_ingress.h",
            "runtime/framework/input/event_ingress.cc",
            "runtime/framework/display/vsync_source.h",
            "runtime/framework/display/vsync_source.cc",
        ],
    )?;
    Ok(ProbeGraphInputs {
        network_probe_inputs,
        hwui_probe_inputs,
        graphics_probe_inputs,
        graphics_gpu_probe_inputs,
        graphics_phase_inputs,
        graphics_input_inputs,
        graphics_state_inputs,
        graphics_session_inputs,
        jni_acceptance_inputs,
        app_bootstrap_inputs,
        app_resources_inputs,
        app_activity_inputs,
        app_presentation_inputs,
        network_probe_stamp,
        hwui_probe_stamp,
        graphics_probe_stamp,
        graphics_gpu_probe_stamp,
        graphics_phase_stamp,
        graphics_input_stamp,
        graphics_state_stamp,
        graphics_session_stamp,
        jni_acceptance_stamp,
        app_bootstrap_stamp,
        app_resources_stamp,
        app_activity_stamp,
        app_presentation_stamp,
        runtime_entry_stamp,
    })
}

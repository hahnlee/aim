use std::ptr;

use crate::ProcessCompletionObserver;
#[cfg(target_os = "macos")]
use crate::appkit_actor::WorkerMessage;
#[cfg(target_os = "macos")]
use crate::bootstrap::attach_runtime;
use crate::config::{HostError, HostOutcome, RunOptions, build_process_request};
use crate::execution::ExecutionImageBinding;
use crate::frame::{FrameHost, receive_frame};
#[cfg(target_os = "macos")]
use crate::gpu_loop::run as run_gpu_loop;
#[cfg(target_os = "macos")]
use crate::host_services::ServiceProcessManager;
use crate::runtime::HostRuntime;
#[cfg(target_os = "macos")]
use crate::teardown::RuntimeShutdownGuard;
#[cfg(target_os = "macos")]
use crate::{process_binder::ApplicationBinderProcess, process_exit::exit_android_process};
#[cfg(target_os = "macos")]
use darwin_art_binder_process::BrokerApi;
#[cfg(target_os = "macos")]
use darwin_art_engine::EngineSession;
use darwin_art_runtime::{ProviderBridge, ProviderKind, Subsystem};
#[cfg(target_os = "macos")]
use std::sync::mpsc::SyncSender;

#[cfg(target_os = "macos")]
use crate::binder_authority::BinderAuthorityBinding;
#[cfg(target_os = "macos")]
use crate::process_filesystem::open_filesystem_authority;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;

pub(crate) fn run_internal(
    options: RunOptions,
    execution_image: Option<ExecutionImageBinding>,
    completion: Option<ProcessCompletionObserver>,
) -> Result<HostOutcome, HostError> {
    #[cfg(target_os = "macos")]
    {
        return crate::appkit_actor::run(options.is_android_process(), move |sender| {
            // Android UI ownership remains on this worker; the Darwin actor
            // handles only scheduling and main-thread event dispatch.
            let qos_status = unsafe {
                libc::pthread_set_qos_class_self_np(
                    libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE,
                    0,
                )
            };
            if qos_status != 0 && std::env::var_os("DARWIN_ART_DEBUG_FRAME_TIMING").is_some() {
                eprintln!("DARWIN_ART owner QoS setup failed status={qos_status}");
            }
            run_owner(
                &options,
                Some(sender),
                execution_image.as_ref(),
                completion.as_ref(),
            )
        });
    }
    #[cfg(not(target_os = "macos"))]
    run_owner(
        &options,
        None,
        execution_image.as_ref(),
        completion.as_ref(),
    )
}

fn run_owner(
    options: &RunOptions,
    #[cfg(target_os = "macos")] appkit_sender: Option<&SyncSender<WorkerMessage>>,
    #[cfg(not(target_os = "macos"))] _appkit_sender: Option<&()>,
    execution_image: Option<&ExecutionImageBinding>,
    completion: Option<&ProcessCompletionObserver>,
) -> Result<HostOutcome, HostError> {
    options.validate()?;

    #[cfg(not(target_os = "macos"))]
    {
        let _ = options;
        let _ = execution_image;
        let _ = completion;
        Err(HostError::UnsupportedPlatform)
    }

    #[cfg(target_os = "macos")]
    {
        let debug_startup = std::env::var_os("DARWIN_ART_DEBUG_PROCESS_CREDENTIALS").is_some();
        // Only the bounded native test loop consumes this cooperative flag.
        // ActivityThread owns an unbounded Android Looper; swallowing SIGTERM
        // there prevents the process supervisor from terminating the app.
        if options.visible_seconds > 0.0 && !options.is_android_process() {
            crate::process_signal::install().map_err(|error| {
                HostError::HostService(format!("install SIGTERM handler: {error}"))
            })?;
        }
        let mut runtime = HostRuntime::new();
        runtime
            .start()
            .map_err(|error| HostError::RuntimeFailed(error.status() as i32))?;
        // Arm cleanup before opening the dynamic image. Every subsequent
        // early return, including loader/provider attach failures, now drops
        // through the same owner-thread shutdown path.
        let mut shutdown_guard =
            RuntimeShutdownGuard::new(&mut runtime, options.execution_lifetime);

        let bootstrap = attach_runtime(
            shutdown_guard.runtime(),
            &options.library,
            execution_image.map(|binding| {
                (
                    binding.path.as_path(),
                    binding.run_symbol.as_str(),
                    binding.shutdown_symbol.as_str(),
                )
            }),
        )
        .inspect_err(|error| eprintln!("ART host bootstrap failed: {error}"))?;
        if debug_startup {
            eprintln!("ART process credentials: runtime bootstrap complete");
        }
        let graphics_attached = bootstrap.graphics_attached;
        if let (Some(sender), Some(engine)) = (appkit_sender, shutdown_guard.runtime().engine()) {
            // SAFETY: hosted execution admits AndroidProcess only. Its shutdown
            // guard never unloads the native image or performs VM teardown;
            // errors exit the process. The main actor seals pumping before
            // authorizing successful process exit. Reusable hosted execution
            // must not reach this registration until retirement is supported.
            let callback = unsafe { engine.appkit_pump_callback() };
            sender
                .send(WorkerMessage::AppKitPump(callback))
                .map_err(|_| HostError::HostService("AppKit actor disconnected".to_owned()))?;
        }

        if let Err(error) = shutdown_guard
            .runtime()
            .install_subsystem(Subsystem::Engine)
        {
            return Err(HostError::RuntimeFailed(error.status() as i32));
        }
        // Provider callbacks point into the live engine image. Install the
        // engine lease before the provider/ELF lease so reverse teardown
        // clears provider hooks while the image is still mapped.
        if let Err(error) = shutdown_guard
            .runtime()
            .install_subsystem(Subsystem::ElfNamespace)
        {
            return Err(HostError::RuntimeFailed(error.status() as i32));
        }
        if graphics_attached
            && let Err(error) = shutdown_guard
                .runtime()
                .install_subsystem(Subsystem::Graphics)
        {
            return Err(HostError::RuntimeFailed(error.status() as i32));
        }
        if debug_startup {
            eprintln!("ART process credentials: subsystem owners installed");
        }

        // Allocate only the desktop scanout target before ActivityThread
        // enters its permanent Looper. Android's ViewRoot/BLAST/HWUI path
        // remains the sole producer of application pixels.
        let app_display = {
            let engine = shutdown_guard
                .runtime()
                .engine()
                .ok_or_else(|| HostError::RuntimeFailed(-1))?;
            crate::app_display::create(engine, options)
                .inspect_err(|error| eprintln!("ART host desktop target failed: {error}"))?
        };
        if let Some(surface) = app_display {
            shutdown_guard
                .runtime()
                .attach_surface(surface)
                .map_err(|_| HostError::RuntimeFailed(-1))?;
            shutdown_guard
                .runtime()
                .install_subsystem(Subsystem::Surface)
                .map_err(|error| HostError::RuntimeFailed(error.status() as i32))?;
        }

        // The native entrypoint receives a Rust-owned lifecycle bridge. It is
        // kept alive through the later shutdown call, so the native entry only
        // reports ART-specific runtime handles and never owns the production
        // phase machine.
        let lifecycle_hooks = shutdown_guard.runtime().native_lifecycle_hooks();
        let mut service_processes =
            ServiceProcessManager::new(options.clone()).map_err(HostError::HostService)?;
        let host_services = service_processes.native_services();

        let mut frame_host = FrameHost {
            frames_received: 0,
            last_frame: None,
        };
        // Install the guest filesystem before Java starts. Libcore can enter
        // UnixNativeDispatcher while constructing the boot class path (for
        // example through Charset.availableCharsets()), well before an APK
        // loads its first native library. The File stays alive through the
        // synchronous Android process invocation; the native facade owns its
        // own duplicate until the Rust process lease is released.
        let filesystem_authority = open_filesystem_authority()?;
        // AOSP application processes retain ProcessState and its Binder pool
        // until the OS reaps the zygote child. These outer owners therefore
        // live through ART, HWUI and the GPU loop and are reclaimed by `_exit`.
        let mut binder_process = options
            .is_android_process()
            .then(ApplicationBinderProcess::new);
        let process = {
            let runtime = shutdown_guard.runtime();
            runtime
                .engine()
                .ok_or(HostError::RuntimeFailed(-1))?
                .install_fd_inheritance_boundary(darwin_art_profile::with_native_operation)
                .map_err(HostError::HostService)?;
            let profile_socket = if options.is_android_process() {
                let socket = std::env::var_os(darwin_art_profile::PROFILE_SOCKET_ENV)
                    .map(std::path::PathBuf::from)
                    .ok_or_else(|| {
                        HostError::HostService(
                            "Android app process has no profile authority socket".into(),
                        )
                    })?;
                crate::process_scm_endpoint::install(
                    runtime.engine().ok_or(HostError::RuntimeFailed(-1))?,
                    &socket,
                )?;
                Some(socket)
            } else {
                None
            };
            let Some(provider) = runtime.provider() else {
                let _ = shutdown_guard.shutdown();
                return Err(HostError::RuntimeFailed(-1));
            };
            if let Some(authority) = filesystem_authority.as_ref() {
                provider
                    .acquire_process_lease(ProviderKind::Filesystem, authority.as_raw_fd())
                    .inspect_err(|error| eprintln!("ART host filesystem lease failed: {error}"))
                    .map_err(HostError::RuntimeFailed)?;
            }
            if debug_startup {
                eprintln!("ART process credentials: filesystem authority installed");
            }
            // Socket and pipe descriptors can arrive in the first Binder
            // transaction that starts an Android service process, before its
            // native library is loaded.  Own the network provider for the
            // whole Android process so SCM_RIGHTS import is available at that
            // bootstrap boundary.  Native-library loads take additional
            // Rust-counted leases without reinstalling the process-global
            // broker.
            provider
                .acquire_process_lease(ProviderKind::Network, -1)
                .map_err(HostError::RuntimeFailed)?;
            if debug_startup {
                eprintln!("ART process credentials: network authority installed");
            }
            // ProcessState maps the Binder receive arena before an APK loads
            // its first native library. Keep Android VM ownership live from
            // process bootstrap so central Binder/ashmem descriptors resolve
            // through the same Bionic mapping boundary at that earlier point.
            // Later native-library loads borrow this Rust-counted lease.
            provider
                .acquire_process_lease(ProviderKind::Vm, -1)
                .map_err(HostError::RuntimeFailed)?;
            if debug_startup {
                eprintln!("ART process credentials: VM authority installed");
            }
            if options.is_android_process() {
                let socket = profile_socket
                    .as_ref()
                    .expect("profile selected before provider acquisition");
                let broker = runtime
                    .engine()
                    .ok_or_else(|| HostError::RuntimeFailed(-1))?
                    .binder_broker_symbols();
                binder_process
                    .as_mut()
                    .expect("APK Binder process boundary armed before acquisition")
                    .connect(
                        &socket,
                        BrokerApi {
                            install_owner: broker.install_owner,
                            publish: broker.publish,
                            uninstall_owner: broker.uninstall_owner,
                            descriptor: darwin_art_binder_process::DescriptorApi {
                                export: broker.export_file,
                                import: broker.import_file,
                                close: broker.close_file,
                                bundle: Some(darwin_art_binder_process::DescriptorBundleApi {
                                    export: broker.export_bound_file,
                                    import: broker.import_bound_file,
                                }),
                                retained: Some(darwin_art_binder_process::DescriptorRetainedApi {
                                    export: broker.export_retained_file,
                                    release: broker.release_export_lease,
                                }),
                            },
                        },
                    )
                    .inspect_err(|error| eprintln!("ART APK Binder startup failed: {error}"))?;
                if debug_startup {
                    eprintln!("ART process credentials: Binder endpoint installed");
                }
            }
            let binder_authority = binder_process.as_ref().and_then(|process| {
                process
                    .authority_lifetime()
                    .map(BinderAuthorityBinding::new)
            });
            let request = match build_process_request(
                options,
                ptr::from_mut(&mut frame_host).cast(),
                Some(receive_frame),
                provider,
                Some(ProviderBridge::acquire_callback()),
                Some(ProviderBridge::release_callback()),
                runtime.graphics(),
                runtime.surface(),
                Some(&lifecycle_hooks),
                Some(&host_services),
                binder_authority.as_ref().map(|binding| binding.hooks()),
            ) {
                Ok(inputs) => inputs,
                Err(error) => {
                    let _ = service_processes.shutdown_all();
                    let _ = shutdown_guard.shutdown();
                    return Err(error);
                }
            };
            let Some(engine) = runtime.engine() else {
                let _ = service_processes.shutdown_all();
                let _ = shutdown_guard.shutdown();
                return Err(HostError::RuntimeFailed(-1));
            };
            if debug_startup {
                eprintln!("ART process credentials: enter ART process request");
            }
            match engine.run_request(&request) {
                Ok(result) => result,
                Err(error) => {
                    eprintln!(
                        "ART host run_request failed status={} before cleanup; service_processes_shutdown=1",
                        error
                    );
                    let _ = service_processes.shutdown_all();
                    let _ = shutdown_guard.shutdown();
                    return Err(HostError::RuntimeFailed(error));
                }
            }
        };

        // The graphics engine publishes its drawable during run_process.
        // Transfer that handle into RuntimeSession immediately, before any
        // later host branch can fail. This keeps the surface owned by the
        // same Rust shutdown transaction as ART/graphics instead of leaving
        // a short-lived foreign owner between process return and the frame
        // loop.
        // app_display may already own this published native surface. Do not
        // create a second wrapper (or reinstall its sink) before discovering
        // that RuntimeSession rejects the duplicate resource attachment.
        let active_surface = if shutdown_guard.runtime().surface().is_some() {
            None
        } else {
            shutdown_guard
                .runtime()
                .engine()
                .and_then(EngineSession::active_surface)
        };
        let has_active_surface = if let Some(surface) = active_surface {
            shutdown_guard
                .runtime()
                .attach_surface(surface)
                .map_err(|_| HostError::RuntimeFailed(-1))?;
            shutdown_guard
                .runtime()
                .install_subsystem(Subsystem::Surface)
                .map_err(|error| HostError::RuntimeFailed(error.status() as i32))?;
            true
        } else {
            shutdown_guard.runtime().surface().is_some()
        };
        if has_active_surface {
            let debug_boundaries = std::env::var_os("DARWIN_ART_DEBUG_FRAME_TIMING").is_some();
            if debug_boundaries {
                eprintln!("DARWIN_ART host: gpu-loop call enter");
            }
            let outcome = run_gpu_loop(
                shutdown_guard.runtime(),
                process,
                options,
                graphics_attached,
            );
            if debug_boundaries {
                eprintln!(
                    "DARWIN_ART host: gpu-loop call exit result={}",
                    outcome.is_ok()
                );
            }
            if options.is_android_process() {
                // An Android app process ends at the OS lifetime boundary.
                // AOSP does not unload the live app NativeLoader graph or
                // destroy ART while Chromium workers may still execute it;
                // reap services and use _exit instead. Embeddable callers
                // take the explicit RuntimeShutdownGuard path below.
                if debug_boundaries {
                    eprintln!("DARWIN_ART host: service cleanup enter mode=terminate");
                }
                let service_cleanup = service_processes
                    .terminate_for_process_exit()
                    .map_err(HostError::HostService);
                if debug_boundaries {
                    eprintln!(
                        "DARWIN_ART host: service cleanup exit mode=terminate ok={}",
                        service_cleanup.is_ok()
                    );
                }
                exit_with_actor_completion(
                    outcome.as_ref(),
                    service_cleanup,
                    completion,
                    appkit_sender,
                );
            }
            // Keep Android Service processes and their Binder channels alive
            // while the browser runtime stops its native/Java threads. Killing
            // renderers first makes Chromium treat an orderly host timeout as
            // an unexpected child death and race its PartitionAlloc teardown.
            if debug_boundaries {
                eprintln!("DARWIN_ART host: runtime cleanup enter");
            }
            let runtime_cleanup = shutdown_guard.shutdown();
            if debug_boundaries {
                eprintln!(
                    "DARWIN_ART host: runtime cleanup exit ok={}",
                    runtime_cleanup.is_ok()
                );
                eprintln!("DARWIN_ART host: service cleanup enter mode=shutdown");
            }
            let service_cleanup = service_processes
                .shutdown_all()
                .map_err(HostError::HostService);
            if debug_boundaries {
                eprintln!(
                    "DARWIN_ART host: service cleanup exit mode=shutdown ok={}",
                    service_cleanup.is_ok()
                );
            }
            return match (outcome, service_cleanup, runtime_cleanup) {
                (Ok(outcome), Ok(()), Ok(())) => Ok(outcome),
                (Err(error), Ok(()), Ok(())) => Err(error),
                (_, Err(error), Ok(())) => Err(error),
                (_, _, Err(error)) => Err(error),
            };
        }

        // Headless ART is a first-class mode. It never allocates a surface
        // and never uploads the callback mailbox into an IOSurface.
        if graphics_attached
            && !shutdown_guard
                .runtime()
                .subsystem_active(Subsystem::Graphics)
            && let Err(error) = shutdown_guard
                .runtime()
                .install_subsystem(Subsystem::Graphics)
        {
            let cleanup = shutdown_guard.shutdown();
            return match cleanup {
                Ok(()) => Err(HostError::RuntimeFailed(error.status() as i32)),
                Err(cleanup_error) => Err(cleanup_error),
            };
        }
        let outcome = HostOutcome {
            process,
            frames_presented: 0,
            last_frame: frame_host.last_frame,
        };
        if options.is_android_process() {
            // See the visible process path above: app-process exit is an OS
            // boundary, not an in-process ART/NativeLoader teardown.
            let service_cleanup = service_processes
                .terminate_for_process_exit()
                .map_err(HostError::HostService);
            exit_with_actor_completion(Ok(&outcome), service_cleanup, completion, appkit_sender);
        }
        shutdown_guard.shutdown()?;
        service_processes
            .shutdown_all()
            .map_err(HostError::HostService)?;
        Ok(outcome)
    }
}

#[cfg(target_os = "macos")]
fn exit_with_actor_completion(
    outcome: Result<&HostOutcome, &HostError>,
    cleanup: Result<(), HostError>,
    completion: Option<&ProcessCompletionObserver>,
    actor: Option<&SyncSender<WorkerMessage>>,
) -> ! {
    if outcome.is_err() || cleanup.is_err() {
        exit_android_process(crate::process_completion::exit_status(
            outcome, cleanup, None,
        ));
    }
    let (request, wait) = crate::process_completion::actor_completion_gate();
    let authorized = actor
        .ok_or_else(|| HostError::HostService("Android process has no AppKit actor".into()))
        .and_then(|actor| {
            actor
                .send(WorkerMessage::ProcessCompletion(request))
                .map_err(|_| {
                    HostError::HostService("AppKit actor disconnected before completion".into())
                })
        })
        .and_then(|_| wait.wait());
    let status = crate::process_completion::exit_status(outcome, authorized, completion);
    exit_android_process(status);
}

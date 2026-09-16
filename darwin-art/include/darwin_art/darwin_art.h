#ifndef DARWIN_ART_DARWIN_ART_H_
#define DARWIN_ART_DARWIN_ART_H_

#include <stddef.h>
#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#define DARWIN_ART_ABI_VERSION 1u
#define DARWIN_ART_NATIVE_LOADER_CONFIG_ABI_VERSION 1u

// Process lifecycle errors are kept outside the existing run-stage values.
// Shutdown is intentionally not idempotent: a duplicate call is reported so
// the host cannot accidentally hide ownership bugs.
#define DARWIN_ART_STATUS_PROCESS_ALREADY_STARTED 66
#define DARWIN_ART_STATUS_SHUTDOWN_NOT_READY 67
#define DARWIN_ART_STATUS_SHUTDOWN_WRONG_THREAD 68
#define DARWIN_ART_STATUS_SHUTDOWN_ALREADY_COMPLETED 69
#define DARWIN_ART_STATUS_SHUTDOWN_FAILED 70

// Graphics-session handles are owner-thread opaque tokens.  They deliberately
// carry no JNI, Skia, or HWUI types across the C ABI; the native side resolves
// the current ART thread and retains those implementation details privately.
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_INVALID 71
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_WRONG_THREAD 72
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_CLOSED 73
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_READY 74
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_ALREADY_ACTIVE 75
#define DARWIN_ART_STATUS_GRAPHICS_SESSION_NOT_CLOSED 76

#if defined(__GNUC__)
#define DARWIN_ART_EXPORT __attribute__((visibility("default")))
#else
#define DARWIN_ART_EXPORT
#endif

// The frame memory is borrowed only for the duration of this callback. The
// host must copy it or retain a separately negotiated surface before returning.
typedef int32_t (*darwin_art_frame_callback_t)(
    void* context,
    const uint32_t* argb_pixels,
    uint32_t width,
    uint32_t height,
    size_t stride_bytes);

// Optional Rust-owned provider lifecycle hooks.  When both callbacks are
// present, provider activation/deactivation is delegated to the host owner;
// when absent, the engine uses its built-in compatibility owner.
typedef int32_t (*darwin_art_provider_acquire_t)(
    void* context, uint32_t provider_kind, int32_t authority_fd);
typedef int32_t (*darwin_art_provider_release_t)(
    void* context, uint32_t provider_kind);

// Optional external lifecycle owner. Production Rust hosts provide these
// callbacks so the native probe cannot become a second phase/lifecycle
// authority. Legacy/direct C callers may leave the pointer null and retain the
// native compatibility state machine.
typedef int32_t (*darwin_art_lifecycle_begin_t)(void* context);
typedef int32_t (*darwin_art_lifecycle_finish_t)(void* context,
                                                  int32_t runtime_created);
typedef void (*darwin_art_lifecycle_failed_t)(void* context, int32_t status);

typedef struct darwin_art_lifecycle_hooks {
  uint32_t struct_size;
  uint32_t abi_version;
  void* context;
  darwin_art_lifecycle_begin_t begin_run;
  darwin_art_lifecycle_finish_t finish_run;
  darwin_art_lifecycle_begin_t begin_shutdown;
  darwin_art_lifecycle_failed_t mark_failed;
} darwin_art_lifecycle_hooks_t;

// Requests a real host process for one Android Service instance. Strings are
// borrowed for the duration of spawn_service. The returned control_fd is a
// connected Unix-domain stream owned by the engine until release_service;
// Android Binder parcels and descriptor rights travel over that stream.
typedef struct darwin_art_service_spawn_request {
  uint32_t struct_size;
  uint32_t abi_version;
  const char* component;
  const char* instance_name;
  const char* process_name;
  int32_t isolated;
} darwin_art_service_spawn_request_t;

typedef struct darwin_art_service_spawn_result {
  uint32_t struct_size;
  uint32_t abi_version;
  int32_t host_pid;
  int32_t control_fd;
} darwin_art_service_spawn_result_t;

typedef int32_t (*darwin_art_spawn_service_t)(
    void* context, const darwin_art_service_spawn_request_t* request,
    darwin_art_service_spawn_result_t* result);
typedef int32_t (*darwin_art_release_service_t)(void* context,
                                                int32_t host_pid);

// Optional Rust-owned ActivityManager process sidecar. Native Android code
// describes the service it needs; process creation, PID ownership, waiting,
// and forced teardown remain entirely in the host.
typedef struct darwin_art_host_services {
  uint32_t struct_size;
  uint32_t abi_version;
  void* context;
  darwin_art_spawn_service_t spawn_service;
  darwin_art_release_service_t release_service;
} darwin_art_host_services_t;

// Optional additive Android NativeLoader input owned by the Rust host. The
// four paths are borrowed for the duration of darwin_art_run_process; callers
// must provide absolute guest/host paths as required by the selected loader
// owner and must not mutate their backing bytes during the call.
typedef struct darwin_art_native_loader_config {
  uint32_t struct_size;
  uint32_t abi_version;
  const char* linker_config_path;
  const char* executable_path;
  const char* library_search_path;
  const char* android_unwind_path;
} darwin_art_native_loader_config_t;

typedef struct darwin_art_process_config {
  uint32_t struct_size;
  uint32_t abi_version;
  const char* core_oj_jar;
  const char* core_libart_jar;
  const char* framework_jar;
  const char* core_icu4j_jar;
  const char* app_dex;
  uint64_t heap_initial_bytes;
  uint64_t heap_maximum_bytes;
  void* host_context;
  darwin_art_frame_callback_t frame_callback;
  void* provider_context;
  darwin_art_provider_acquire_t provider_acquire;
  darwin_art_provider_release_t provider_release;
  // Optional additive graphics-session sidecar. Older callers may provide a
  // legacy struct prefix without this field; the engine checks struct_size
  // before reading it and never aliases host_context for graphics state.
  void* graphics_session_context;
  // Optional additive Rust lifecycle owner. Older callers may provide a
  // prefix ending at graphics_session_context.
  const darwin_art_lifecycle_hooks_t* lifecycle_hooks;
  // Optional additive Rust ActivityManager/process owner. Older callers may
  // provide a prefix ending at lifecycle_hooks.
  const darwin_art_host_services_t* host_services;
  // Optional additive NativeLoader configuration. Older callers may provide a
  // prefix ending at host_services; the engine checks struct_size before use.
  const darwin_art_native_loader_config_t* native_loader_config;
} darwin_art_process_config_t;

typedef struct darwin_art_process_result {
  uint32_t struct_size;
  uint32_t abi_version;
  int32_t hello_answer;
  int32_t native_round_trip;
  int32_t arraycopy_result;
  int32_t activity_probe_result;
  int32_t lifecycle_result;
  uint32_t frame_width;
  uint32_t frame_height;
} darwin_art_process_result_t;

typedef struct darwin_art_graphics_session_t darwin_art_graphics_session_t;

// Runs exactly one ART instance in the current process. Version 1 deliberately
// exposes a one-shot operation: Runtime::Create is process-global, and the
// existing Darwin port does not yet promise safe re-creation after shutdown.
// A return value of zero is success; nonzero values are stable process-run
// stages and are also diagnosed on stderr.
DARWIN_ART_EXPORT int32_t darwin_art_run_process(
    const darwin_art_process_config_t* config,
    darwin_art_process_result_t* result);

// Dispatches one host pointer sample into the retained Android DecorView and
// emits a refreshed frame. Actions are 0=down, 1=up, 2=move, and 3=cancel.
// APK runs use the versioned MotionEvent/DecorView bridge; direct hit-testing
// remains available only as an explicit compatibility fallback.
DARWIN_ART_EXPORT int32_t darwin_art_dispatch_pointer(
    uint32_t action,
    float x,
    float y);

// Tears down the Runtime created by darwin_art_run_process. Once Runtime::Create
// succeeded, the host must call this exactly once after run_process returns,
// including when run_process returned a nonzero stage. It must be called on the
// same native thread; frame callbacks must return before shutdown begins.
//
// The function deletes outstanding process global references, destroys ART,
// and only then releases the registered application DexFile storage. A zero
// return is success. NOT_READY means no live Runtime exists (including a create
// failure); WRONG_THREAD leaves it available for the owning thread; an
// ALREADY_COMPLETED result identifies a duplicate shutdown call.
DARWIN_ART_EXPORT int32_t darwin_art_shutdown_process(void);

// Creates an owner-thread graphics handle around the HWUI state initialized
// by darwin_art_run_process.  The handle is intentionally additive to the
// process ABI: callers may adopt it incrementally without passing it through
// darwin_art_process_config_t::host_context.  A null return means the ART
// owner thread has not reached the graphics-ready state or another session is
// still active.
DARWIN_ART_EXPORT darwin_art_graphics_session_t*
darwin_art_graphics_session_create(void);

// Closes the HWUI/JNI state owned by the handle.  This must run on the ART
// owner thread.  Close is separate from destroy so a Rust owner can make the
// lifetime transition explicit and reject use-after-close deterministically.
DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_close(
    darwin_art_graphics_session_t* session);

// Releases an already-closed handle.  Destroying an open handle is rejected
// and leaves ownership with the caller, preventing an accidental leak of the
// native HWUI state hidden behind a premature free.
DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_destroy(
    darwin_art_graphics_session_t* session);

DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_dispatch_pointer(
    darwin_art_graphics_session_t* session, uint32_t action, float x, float y);

DARWIN_ART_EXPORT int32_t darwin_art_graphics_session_pump_frame(
    darwin_art_graphics_session_t* session, int64_t frame_time_nanos);

#if defined(__cplusplus)
}  // extern "C"
#endif

#endif  // DARWIN_ART_DARWIN_ART_H_

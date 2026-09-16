#pragma once

#include <cstdint>

// Hooks consumed only by the Darwin build of AOSP Binder RPC/JNI. Session is
// an opaque RpcSession identity; credentials always come from its accepted FD.
extern "C" bool darwin_art_binder_rpc_identity_bind(const void* session,
                                                     int socket_fd);
extern "C" void darwin_art_binder_rpc_identity_forget(const void* session);
extern "C" bool darwin_art_binder_rpc_identity_enter(const void* session,
                                                      uint32_t flags);
extern "C" void darwin_art_binder_rpc_identity_leave();
extern "C" bool darwin_art_binder_rpc_identity_current(int32_t* pid,
                                                        int32_t* uid,
                                                        bool* explicit_identity);
// Effective Binder identity for the current thread. Unlike `current`, this
// also returns the registered process identity outside an incoming RPC call,
// matching IPCThreadState's local-call behavior without opening /dev/binder.
extern "C" bool darwin_art_binder_rpc_identity_effective(
    int32_t* pid, int32_t* uid, bool* explicit_identity);
extern "C" bool darwin_art_binder_rpc_identity_clear(int64_t* token);
extern "C" bool darwin_art_binder_rpc_identity_restore(int64_t token);
// RPC Binder carries the same per-thread StrictMode/work-source state that
// IPCThreadState owns for the kernel driver. Darwin has no kernel Binder
// driver, so this thread state also covers local Binder calls outside an
// authenticated incoming RPC transaction.
extern "C" bool darwin_art_binder_rpc_strict_mode_get(int32_t* policy);
extern "C" bool darwin_art_binder_rpc_strict_mode_set(int32_t policy);
extern "C" bool darwin_art_binder_rpc_work_source_set(int32_t uid,
                                                       int64_t* token);
extern "C" bool darwin_art_binder_rpc_work_source_get(int32_t* uid);
extern "C" bool darwin_art_binder_rpc_work_source_clear(int64_t* token);
extern "C" bool darwin_art_binder_rpc_work_source_restore(int64_t token);

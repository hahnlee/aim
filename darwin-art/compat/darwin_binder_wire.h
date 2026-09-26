#pragma once

#include <jni.h>

#include <string>
#include <cstdint>
#include <memory>

namespace darwin_art {
namespace binder { class WireChannelLifetime; }

// Register transport on a runtime RemoteBinder class from the caller's loader.
// Needed both for app-created endpoints and Binder-imported system callbacks.
bool RegisterRemoteBinderNatives(JNIEnv* env, jclass endpoint);
bool RegisterSystemServicesNatives(JNIEnv* env, jclass endpoint);
jobject ConnectSystemBinder(JNIEnv* env, const char* socket_path);

// Starts the process-local Binder reader used to receive callbacks from a
// remote Android Service after the initiating Java transact() has returned.
bool StartRemoteBinderDispatcher(JNIEnv* env, jint control_fd);

// Publishes the child Service binder and moves socket dispatch onto an
// attached Binder worker. The service owner thread must remain in its Android
// Looper, matching ActivityThread rather than blocking on the transport.
bool StartServingRemoteBinder(JNIEnv* env, jint control_fd,
                              jobject local_binder);

// Sends the original Context.bindService() Intent to a newly spawned service
// process using the same versioned Parcel/Binder/FD transport as transactions.
bool SendServiceBindIntent(JNIEnv* env, jint control_fd, jobject intent);

// Receives and unparcels the original bind Intent in the service process.
// The returned reference is local to the caller's JNI frame.
jobject ReceiveServiceBindIntent(JNIEnv* env, jint control_fd);

// Transports an Android Binder transaction over the service process channel.
// The Parcel byte stream, Binder object table, and owned file descriptors are
// transferred as one versioned message; descriptors use SCM_RIGHTS.
jboolean TransactRemoteBinder(JNIEnv* env, jint control_fd, uint64_t generation, jint target_id,
                              jint code, jobject data, jobject reply,
                              jint flags);

// Returns only the exact still-established generation. It never creates a
// channel or restores a retired lifetime from a numeric file descriptor.
std::shared_ptr<binder::WireChannelLifetime> FindRemoteBinderChannelLifetime(
    jint control_fd, uint64_t generation);
// Resource-owner metadata port, not a proxy authority lookup. Call only while
// the caller owns the established channel descriptor.
std::shared_ptr<binder::WireChannelLifetime>
CaptureEstablishedRemoteBinderChannelLifetime(jint control_fd);

// Legacy synchronous child endpoint. New service processes should use
// StartServingRemoteBinder and keep their owner thread in Looper.loop().
int ServeRemoteBinder(JNIEnv* env, jint control_fd, jobject local_binder);

// Releases browser-side global Binder references associated with a channel.
void CloseRemoteBinderChannel(JNIEnv* env, jint control_fd);


}  // namespace darwin_art

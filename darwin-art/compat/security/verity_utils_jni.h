#pragma once
#include <jni.h>

#include <cstdint>

namespace darwin_art::security {
// com.android.internal.security.VerityUtils natives: fs-verity digests kept
// for files under the private guest /data mounts.
bool RegisterVerityUtilsNatives(JNIEnv* env);
// The fs-verity file digest (SHA-256, 4 KiB blocks, no salt) of `content`,
// as FS_IOC_MEASURE_VERITY reports it; `digest` receives 32 bytes.
void ComputeFsverityDigest(const uint8_t* content, uint64_t size, uint8_t* digest);
}

#include "verity_utils_jni.h"

#include <CommonCrypto/CommonDigest.h>
#include <fcntl.h>
#include <limits.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>

#include <cerrno>
#include <cstdint>
#include <cstring>
#include <vector>

#include "../darwin_libcore_filesystem_bridge.h"

extern "C" int darwin_art_bionic_fs_resolve_private_host_path(const char*, char*, size_t)
    __attribute__((weak_import));
extern "C" long darwin_art_bionic_fs_fd_private_host_path(int, char*, size_t)
    __attribute__((weak_import));

// com_android_internal_security_VerityUtils.cpp at the Darwin boundary. The
// host filesystem has no fs-verity, so the digest the kernel would compute
// (SHA-256 over a 4 KiB-block Merkle tree, then over the fsverity_descriptor)
// is computed here when a file under a private guest /data mount is enabled.
// It is kept, with the file's size and modification time, in a host-private
// extended attribute that the guest's user.* namespace never shows. Linux
// forbids writes to a verity file; here a later write clears the protection,
// because the stored stamp no longer matches, instead of leaving a stale
// digest. Paths outside the private mounts keep the non-verity filesystem
// answers (EOPNOTSUPP), as before.
namespace darwin_art::security {
namespace {
// Android (Linux) errno values returned to Java.
constexpr jint kAndroidEinval = 22;
constexpr jint kAndroidEexist = 17;
constexpr jint kAndroidEopnotsupp = 95;
constexpr jint kAndroidEbadf = 9;
constexpr jint kAndroidEio = 5;

constexpr size_t kBlockSize = 4096;
constexpr size_t kDigestSize = CC_SHA256_DIGEST_LENGTH;
constexpr char kVerityAttribute[] = "dev.darwinart.fsverity";

struct VerityRecord {
  uint8_t digest[kDigestSize];
  uint64_t size;
  int64_t mtime_seconds;
  int64_t mtime_nanoseconds;
};

jint AndroidErrno(int darwin_errno) {
  // ENOENT, EACCES, EBADF, ENOTDIR, EISDIR and the other low values match.
  return darwin_errno >= 1 && darwin_errno <= 34 ? darwin_errno : kAndroidEinval;
}

void Sha256(const uint8_t* data, size_t size, uint8_t* out) {
  CC_SHA256(data, static_cast<CC_LONG>(size), out);
}

}  // namespace

// The fs-verity file digest (FS_IOC_MEASURE_VERITY, SHA-256, 4 KiB blocks,
// no salt) of {@code content}.
void ComputeFsverityDigest(const uint8_t* content, uint64_t size, uint8_t* digest) {
  uint8_t root[64] = {};
  if (size != 0) {
    // Level 0: one hash per data block, the last one zero-padded.
    std::vector<uint8_t> level;
    std::vector<uint8_t> block(kBlockSize);
    for (uint64_t offset = 0; offset < size; offset += kBlockSize) {
      const size_t length = static_cast<size_t>(
          size - offset < kBlockSize ? size - offset : kBlockSize);
      std::memset(block.data(), 0, kBlockSize);
      std::memcpy(block.data(), content + offset, length);
      uint8_t hash[kDigestSize];
      Sha256(block.data(), kBlockSize, hash);
      level.insert(level.end(), hash, hash + kDigestSize);
    }
    // Pack each level's hashes into zero-padded blocks and hash those until
    // one hash remains: the root hash (a one-block file's is its block hash).
    while (level.size() > kDigestSize) {
      std::vector<uint8_t> next;
      for (size_t offset = 0; offset < level.size(); offset += kBlockSize) {
        const size_t length = level.size() - offset < kBlockSize ? level.size() - offset
                                                                 : kBlockSize;
        std::memset(block.data(), 0, kBlockSize);
        std::memcpy(block.data(), level.data() + offset, length);
        uint8_t hash[kDigestSize];
        Sha256(block.data(), kBlockSize, hash);
        next.insert(next.end(), hash, hash + kDigestSize);
      }
      level.swap(next);
    }
    std::memcpy(root, level.data(), kDigestSize);
  }
  // struct fsverity_descriptor (256 bytes, little endian).
  uint8_t descriptor[256] = {};
  descriptor[0] = 1;   // version
  descriptor[1] = 1;   // FS_VERITY_HASH_ALG_SHA256
  descriptor[2] = 12;  // log2(4096)
  descriptor[3] = 0;   // salt size
  for (int i = 0; i < 8; ++i) descriptor[8 + i] = static_cast<uint8_t>(size >> (8 * i));
  std::memcpy(descriptor + 16, root, sizeof(root));
  Sha256(descriptor, sizeof(descriptor), digest);
}

namespace {

// The host path of a guest path under a private /data mount, or an Android errno.
jint HostPath(JNIEnv* env, jstring path, char* host, size_t capacity) {
  if (path == nullptr) return kAndroidEinval;
  const char* text = env->GetStringUTFChars(path, nullptr);
  if (text == nullptr) return kAndroidEinval;
  struct stat status {};
  jint result = 0;
  if (darwin_art_libcore_stat(text, &status) != 0) {
    result = AndroidErrno(errno);
  } else if (darwin_art_bionic_fs_resolve_private_host_path == nullptr ||
             darwin_art_bionic_fs_resolve_private_host_path(text, host, capacity) < 0) {
    result = kAndroidEopnotsupp;
  }
  env->ReleaseStringUTFChars(path, text);
  return result;
}

bool Current(const char* host, const VerityRecord& record) {
  struct stat status {};
  return stat(host, &status) == 0 && static_cast<uint64_t>(status.st_size) == record.size &&
         status.st_mtimespec.tv_sec == record.mtime_seconds &&
         status.st_mtimespec.tv_nsec == record.mtime_nanoseconds;
}

// 1 with {@code record} filled when the host file has current protection,
// 0 when it has none, -errno on failure.
int ReadRecord(const char* host, VerityRecord* record) {
  const ssize_t length =
      getxattr(host, kVerityAttribute, record, sizeof(*record), 0, XATTR_NOFOLLOW);
  if (length < 0) return errno == ENOATTR ? 0 : -AndroidErrno(errno);
  if (length != sizeof(*record)) return 0;
  if (Current(host, *record)) return 1;
  // Written since it was enabled: the protection no longer applies.
  removexattr(host, kVerityAttribute, XATTR_NOFOLLOW);
  return 0;
}

jint EnableHost(const char* host) {
  VerityRecord record{};
  const int existing = ReadRecord(host, &record);
  if (existing < 0) return -existing;
  if (existing > 0) return kAndroidEexist;
  const int fd = open(host, O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
  if (fd < 0) return AndroidErrno(errno);
  struct stat status {};
  if (fstat(fd, &status) != 0 || !S_ISREG(status.st_mode)) {
    close(fd);
    return kAndroidEinval;
  }
  std::vector<uint8_t> content(static_cast<size_t>(status.st_size));
  size_t done = 0;
  while (done < content.size()) {
    const ssize_t count = read(fd, content.data() + done, content.size() - done);
    if (count < 0 && errno == EINTR) continue;
    if (count <= 0) {
      close(fd);
      return kAndroidEio;
    }
    done += static_cast<size_t>(count);
  }
  close(fd);
  ComputeFsverityDigest(content.data(), content.size(), record.digest);
  record.size = static_cast<uint64_t>(status.st_size);
  record.mtime_seconds = status.st_mtimespec.tv_sec;
  record.mtime_nanoseconds = status.st_mtimespec.tv_nsec;
  if (setxattr(host, kVerityAttribute, &record, sizeof(record), 0, XATTR_NOFOLLOW) != 0) {
    return AndroidErrno(errno);
  }
  return 0;
}

// A guest descriptor on the private data mount (ResilientAtomicFile's
// settings writes); other descriptors keep the non-verity answer.
jint EnableForFd(JNIEnv*, jclass, jint fd) {
  if (fd < 0) return kAndroidEbadf;
  char host[PATH_MAX];
  if (darwin_art_bionic_fs_fd_private_host_path == nullptr ||
      darwin_art_bionic_fs_fd_private_host_path(fd, host, sizeof(host)) < 0) {
    return kAndroidEopnotsupp;
  }
  return EnableHost(host);
}

jint Enable(JNIEnv* env, jclass, jstring path) {
  char host[PATH_MAX];
  const jint resolved = HostPath(env, path, host, sizeof(host));
  return resolved != 0 ? resolved : EnableHost(host);
}

// 1 when the file has fs-verity, 0 when not, -errno on error.
jint Statx(JNIEnv* env, jclass, jstring path) {
  char host[PATH_MAX];
  const jint resolved = HostPath(env, path, host, sizeof(host));
  if (resolved == kAndroidEopnotsupp) return 0;
  if (resolved != 0) return -resolved;
  VerityRecord record{};
  return ReadRecord(host, &record);
}

jint Measure(JNIEnv* env, jclass, jstring path, jbyteArray digest) {
  char host[PATH_MAX];
  const jint resolved = HostPath(env, path, host, sizeof(host));
  if (resolved != 0) return -resolved;
  VerityRecord record{};
  const int protection = ReadRecord(host, &record);
  if (protection < 0) return protection;
  // FS_IOC_MEASURE_VERITY on a file without fs-verity.
  if (protection == 0) return -61;  // ENODATA
  if (digest == nullptr || env->GetArrayLength(digest) < static_cast<jsize>(kDigestSize)) {
    return -kAndroidEinval;
  }
  env->SetByteArrayRegion(digest, 0, kDigestSize, reinterpret_cast<const jbyte*>(record.digest));
  return 0;
}
}  // namespace

bool RegisterVerityUtilsNatives(JNIEnv* env) {
  jclass verity = env->FindClass("com/android/internal/security/VerityUtils");
  if (verity == nullptr) return false;
  const JNINativeMethod methods[] = {
      {const_cast<char*>("enableFsverityNative"), const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(Enable)},
      {const_cast<char*>("enableFsverityForFdNative"), const_cast<char*>("(I)I"),
       reinterpret_cast<void*>(EnableForFd)},
      {const_cast<char*>("statxForFsverityNative"), const_cast<char*>("(Ljava/lang/String;)I"),
       reinterpret_cast<void*>(Statx)},
      {const_cast<char*>("measureFsverityNative"), const_cast<char*>("(Ljava/lang/String;[B)I"),
       reinterpret_cast<void*>(Measure)},
  };
  const bool registered = env->RegisterNatives(verity, methods, 4) == JNI_OK;
  env->DeleteLocalRef(verity);
  return registered;
}
}  // namespace darwin_art::security

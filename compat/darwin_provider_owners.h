#ifndef AIM_PROVIDER_OWNERS_H_
#define AIM_PROVIDER_OWNERS_H_

#include <cstdint>
#include <string>

namespace aim::providers {

enum class Kind : uint32_t {
  Filesystem = 1,
  Network = 2,
  Stdio = 3,
  Ioctl = 4,
  Strftime = 5,
  Sendfile = 6,
  Vm = 7,
};

using AcquireHook = int32_t (*)(void* context, uint32_t kind, int32_t authority_fd);
using ReleaseHook = int32_t (*)(void* context, uint32_t kind);

extern "C" void aim_provider_install_hooks(
    void* context, AcquireHook acquire, ReleaseHook release);
extern "C" void aim_provider_clear_hooks();

// Native direct entrypoints bypass the installed Rust hook. They are the
// capability-backed implementation used by the Rust owner callback.
extern "C" int32_t aim_provider_native_acquire(uint32_t kind,
                                                       int32_t authority_fd);
extern "C" int32_t aim_provider_native_release(uint32_t kind);

bool acquire_filesystem(int directory_fd, std::string* error);
bool seed_filesystem_private_directory(const char* guest_path,
                                       std::string* error);
void release_filesystem();

bool acquire_network(std::string* error);
void release_network();

bool acquire_stdio(std::string* error);
void release_stdio();

bool acquire_ioctl(std::string* error);
void release_ioctl();

bool acquire_strftime(std::string* error);
void release_strftime();

bool acquire_sendfile(std::string* error);
void release_sendfile();

bool acquire_vm(std::string* error);
void release_vm();

}  // namespace aim::providers

#endif  // AIM_PROVIDER_OWNERS_H_

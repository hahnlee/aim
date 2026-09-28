#include "aim_bionic_wide_stdio.h"

#include "aim_bionic_locale.h"

#include <condition_variable>
#include <cstring>
#include <memory>
#include <mutex>
#include <new>
#include <unordered_map>

extern "C" void aim_bionic_errno_store(int value);

namespace {

constexpr int kEinval = 22;
constexpr int kEnomem = 12;
constexpr int kEio = 5;

struct WideState {
  std::mutex mutex;
  AimAndroidMbState input{};
  AimAndroidMbState output{};
  uint32_t pushback = 0;
  bool has_pushback = false;
};

struct Backend {
  explicit Backend(const AimBionicWideStdioBackendV1& value)
      : operations(value) {}

  AimBionicWideStdioBackendV1 operations;
  std::mutex states_mutex;
  std::unordered_map<AimAndroidFile*, std::shared_ptr<WideState>> states;
  std::mutex lifetime_mutex;
  std::condition_variable lifetime_changed;
  size_t active_calls = 0;
  bool closing = false;
};

struct AimBionicWideStdioActivationImpl {
  std::shared_ptr<Backend> backend;
};

std::mutex g_active_mutex;
std::shared_ptr<Backend> g_active;

void StoreErrno(int value) {
  aim_bionic_errno_store(value);
}

class Call {
 public:
  static Call Begin() {
    std::shared_ptr<Backend> backend;
    {
      std::lock_guard<std::mutex> lock(g_active_mutex);
      backend = g_active;
    }
    if (!backend) {
      StoreErrno(kEio);
      return Call();
    }
    {
      std::lock_guard<std::mutex> lock(backend->lifetime_mutex);
      if (backend->closing) {
        StoreErrno(kEio);
        return Call();
      }
      ++backend->active_calls;
    }
    return Call(std::move(backend));
  }

  Call() = default;
  Call(const Call&) = delete;
  Call& operator=(const Call&) = delete;
  Call(Call&& other) noexcept : backend_(std::move(other.backend_)) {}
  Call& operator=(Call&&) = delete;
  ~Call() {
    if (!backend_) return;
    std::lock_guard<std::mutex> lock(backend_->lifetime_mutex);
    if (--backend_->active_calls == 0) backend_->lifetime_changed.notify_all();
  }

  explicit operator bool() const { return backend_ != nullptr; }
  Backend* operator->() const { return backend_.get(); }
  Backend& operator*() const { return *backend_; }

 private:
  explicit Call(std::shared_ptr<Backend> backend)
      : backend_(std::move(backend)) {}
  std::shared_ptr<Backend> backend_;
};

class Lease {
 public:
  Lease(Backend& backend, AimAndroidFile* file) : backend_(backend) {
    if (file != nullptr &&
        backend_.operations.acquire(backend_.operations.context, file,
                                    &value_) == 0 &&
        value_ != nullptr) {
      acquired_ = true;
    }
  }
  Lease(const Lease&) = delete;
  Lease& operator=(const Lease&) = delete;
  ~Lease() {
    if (acquired_) {
      backend_.operations.release(backend_.operations.context, value_);
    }
  }
  explicit operator bool() const { return acquired_; }
  void* get() const { return value_; }

 private:
  Backend& backend_;
  void* value_ = nullptr;
  bool acquired_ = false;
};

bool ValidBackend(const AimBionicWideStdioBackendV1* backend) {
  return backend != nullptr &&
         backend->abi_version == AIM_BIONIC_WIDE_STDIO_BACKEND_ABI &&
         backend->struct_size >= sizeof(*backend) && backend->acquire != nullptr &&
         backend->release != nullptr && backend->orient_wide != nullptr &&
         backend->read_byte != nullptr && backend->write_bytes != nullptr &&
         backend->set_error != nullptr &&
         backend->clear_error_and_eof != nullptr;
}

std::shared_ptr<WideState> GetState(Backend& backend,
                                    AimAndroidFile* file) {
  std::lock_guard<std::mutex> lock(backend.states_mutex);
  auto found = backend.states.find(file);
  if (found != backend.states.end()) return found->second;
  auto state = std::make_shared<WideState>();
  backend.states.emplace(file, state);
  return state;
}

bool Prepare(Call& call,
             AimAndroidFile* file,
             std::unique_ptr<Lease>* lease) {
  if (!call) return false;
  auto candidate = std::make_unique<Lease>(*call, file);
  if (!*candidate) return false;
  if (call->operations.orient_wide(call->operations.context,
                                   candidate->get()) != 0) {
    return false;
  }
  *lease = std::move(candidate);
  return true;
}

bool Equal(const char* left, const char* right) {
  return left != nullptr && right != nullptr && std::strcmp(left, right) == 0;
}

}  // namespace

extern "C" AimBionicWideStdioActivation*
aim_bionic_wide_stdio_install(
    const AimBionicWideStdioBackendV1* operations) noexcept {
  if (!ValidBackend(operations)) {
    StoreErrno(kEinval);
    return nullptr;
  }
  try {
    auto backend = std::make_shared<Backend>(*operations);
    auto activation = std::make_unique<AimBionicWideStdioActivationImpl>();
    activation->backend = backend;
    {
      std::lock_guard<std::mutex> lock(g_active_mutex);
      if (g_active) {
        StoreErrno(kEinval);
        return nullptr;
      }
      g_active = std::move(backend);
    }
    return reinterpret_cast<AimBionicWideStdioActivation*>(
        activation.release());
  } catch (const std::bad_alloc&) {
    StoreErrno(kEnomem);
  } catch (...) {
    StoreErrno(kEio);
  }
  return nullptr;
}

extern "C" int aim_bionic_wide_stdio_uninstall(
    AimBionicWideStdioActivation** activation) noexcept {
  if (activation == nullptr || *activation == nullptr) {
    StoreErrno(kEinval);
    return -1;
  }
  auto owned = std::unique_ptr<AimBionicWideStdioActivationImpl>(
      reinterpret_cast<AimBionicWideStdioActivationImpl*>(*activation));
  *activation = nullptr;
  {
    std::lock_guard<std::mutex> lock(g_active_mutex);
    if (g_active != owned->backend) {
      StoreErrno(kEinval);
      return -1;
    }
    g_active.reset();
  }
  std::unique_lock<std::mutex> lock(owned->backend->lifetime_mutex);
  owned->backend->closing = true;
  owned->backend->lifetime_changed.wait(
      lock, [&] { return owned->backend->active_calls == 0; });
  return 0;
}

extern "C" int aim_bionic_wide_stdio_reset(
    AimAndroidFile* file) noexcept {
  try {
    Call call = Call::Begin();
    if (!call || file == nullptr) return -1;
    std::lock_guard<std::mutex> lock(call->states_mutex);
    call->states.erase(file);
    return 0;
  } catch (...) {
    StoreErrno(kEio);
    return -1;
  }
}

extern "C" int aim_bionic_wide_stdio_forget(
    AimAndroidFile* file) noexcept {
  return aim_bionic_wide_stdio_reset(file);
}

extern "C" uint32_t aim_bionic_wide_stdio_fputwc_core(
    uint32_t wc,
    AimAndroidFile* file) noexcept {
  try {
    Call call = Call::Begin();
    std::unique_ptr<Lease> lease;
    if (!Prepare(call, file, &lease)) return AIM_BIONIC_WEOF;
    std::shared_ptr<WideState> state = GetState(*call, file);
    if (!state) return AIM_BIONIC_WEOF;
    std::lock_guard<std::mutex> lock(state->mutex);
    state->has_pushback = false;
    char encoded[4]{};
    size_t size = aim_bionic_wcrtomb(encoded, wc, &state->output);
    if (size == static_cast<size_t>(-1)) {
      call->operations.set_error(call->operations.context, lease->get());
      return AIM_BIONIC_WEOF;
    }
    if (call->operations.write_bytes(
            call->operations.context, lease->get(),
            reinterpret_cast<const uint8_t*>(encoded), size) != 0) {
      return AIM_BIONIC_WEOF;
    }
    return wc;
  } catch (const std::bad_alloc&) {
    StoreErrno(kEnomem);
  } catch (...) {
    StoreErrno(kEio);
  }
  return AIM_BIONIC_WEOF;
}

extern "C" uint32_t aim_bionic_wide_stdio_getwc_core(
    AimAndroidFile* file) noexcept {
  try {
    Call call = Call::Begin();
    std::unique_ptr<Lease> lease;
    if (!Prepare(call, file, &lease)) return AIM_BIONIC_WEOF;
    std::shared_ptr<WideState> state = GetState(*call, file);
    if (!state) return AIM_BIONIC_WEOF;
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->has_pushback) {
      state->has_pushback = false;
      return state->pushback;
    }
    for (;;) {
      uint8_t byte = 0;
      int read = call->operations.read_byte(call->operations.context,
                                            lease->get(), &byte);
      if (read <= 0) return AIM_BIONIC_WEOF;
      uint32_t output = 0;
      char input = static_cast<char>(byte);
      size_t size = aim_bionic_mbrtowc(&output, &input, 1,
                                               &state->input);
      if (size == static_cast<size_t>(-2)) continue;
      if (size == static_cast<size_t>(-1)) {
        call->operations.set_error(call->operations.context, lease->get());
        return AIM_BIONIC_WEOF;
      }
      return output;
    }
  } catch (const std::bad_alloc&) {
    StoreErrno(kEnomem);
  } catch (...) {
    StoreErrno(kEio);
  }
  return AIM_BIONIC_WEOF;
}

extern "C" uint32_t aim_bionic_wide_stdio_ungetwc_core(
    uint32_t wc,
    AimAndroidFile* file) noexcept {
  if (wc == AIM_BIONIC_WEOF) return AIM_BIONIC_WEOF;
  try {
    Call call = Call::Begin();
    std::unique_ptr<Lease> lease;
    if (!Prepare(call, file, &lease)) return AIM_BIONIC_WEOF;
    std::shared_ptr<WideState> state = GetState(*call, file);
    if (!state) return AIM_BIONIC_WEOF;
    std::lock_guard<std::mutex> lock(state->mutex);
    if (state->has_pushback) {
      return AIM_BIONIC_WEOF;
    }
    state->pushback = wc;
    state->has_pushback = true;
    call->operations.clear_error_and_eof(call->operations.context,
                                         lease->get());
    return wc;
  } catch (const std::bad_alloc&) {
    StoreErrno(kEnomem);
  } catch (...) {
    StoreErrno(kEio);
  }
  return AIM_BIONIC_WEOF;
}

extern "C" AimBionicWideStdioFunction
aim_bionic_wide_stdio_resolve(const char* soname,
                                     const char* symbol,
                                     const char* version) noexcept {
  if (!Equal(soname, "libc.so") || !Equal(version, "LIBC") || symbol == nullptr) {
    return nullptr;
  }
  if (std::strcmp(symbol, "fputwc") == 0) {
    return reinterpret_cast<AimBionicWideStdioFunction>(
        aim_bionic_fputwc);
  }
  if (std::strcmp(symbol, "getwc") == 0) {
    return reinterpret_cast<AimBionicWideStdioFunction>(
        aim_bionic_getwc);
  }
  if (std::strcmp(symbol, "ungetwc") == 0) {
    return reinterpret_cast<AimBionicWideStdioFunction>(
        aim_bionic_ungetwc);
  }
  return nullptr;
}

#include "proxy_vm.h"
#include <utility>

namespace aim::jni {
std::shared_ptr<ProxyVm> ProxyVm::Create(std::shared_ptr<void> context,
    const AimJniBackend& backend, std::string* error) try {
  if (error) error->clear();
  if (!context || backend.context != context.get() || !backend.current_env) {
    if (error) *error = "proxy VM requires an owned backend context/current env";
    return nullptr;
  }
  auto vm = std::shared_ptr<ProxyVm>(new ProxyVm());
  vm->proxy_ = aim_jni_proxy_init(vm->storage_.data(), vm->storage_.size(), &backend);
  if (!vm->proxy_) {
    if (error) *error = "incomplete proxy VM backend";
    return nullptr;
  }
  vm->context_ = std::move(context);
  return vm;
} catch (...) {
  if (error) *error = "cannot allocate proxy VM owner";
  return nullptr;
}

void* ProxyVm::JavaVm() const { return aim_jni_proxy_java_vm(proxy_); }
}

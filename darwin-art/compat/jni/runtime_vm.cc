#include "runtime_vm.h"
#include "art_registration.h"

namespace darwin_art::jni {
std::unique_ptr<RuntimeVm> RuntimeVm::Create(JavaVM* art_vm,
    std::shared_ptr<loader::NamespaceHandles> namespaces, std::string* error) try {
  if (error) error->clear();
  if (!art_vm || !namespaces) {
    if (error) *error = "JNI VM requires ART and its shared process namespaces";
    return nullptr;
  }
  auto result = std::unique_ptr<RuntimeVm>(new RuntimeVm());
  auto context = VmContext::Create(art_vm, std::make_shared<ArtRegistration>());
  if (!context) return nullptr;
  result->proxy_ = ProxyVm::Create(context, context->Backend(), error);
  if (!result->proxy_) return nullptr;
  result->methods_ = RegisteredMethods::Create(std::move(namespaces), result->proxy_);
  if (!result->methods_) {
    if (error) *error = "cannot create ART registered method owner";
    return nullptr;
  }
  result->art_vm_ = art_vm;
  return result;
} catch (...) {
  // Avoid allocating an error string while already handling allocation failure.
  return nullptr;
}

RuntimeVm::~RuntimeVm() = default;

DarwinArtRegisteredNativeResolution RuntimeVm::Resolve(const void* function,
    bool bridged, const char* shorty, uint32_t length, DarwinArtJniCallType kind,
    const void** out) {
  return methods_->Resolve(function, bridged, shorty, length, kind, out);
}

void RuntimeVm::RetireGroup(uint64_t group) { methods_->RetireGroup(group); }
}

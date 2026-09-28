#pragma once
#include "aim_bionic_provider_namespace.h"
#include <memory>
#include <string>

namespace aim::loader {
struct BionicProviderDrop {
  void operator()(AimBionicNamespace*) const;
};
using BionicProviderSet = std::unique_ptr<AimBionicNamespace, BionicProviderDrop>;
// Owns setup until caller transfers it into the Rust runtime native owner.
// A failed bind/seal never publishes a partial provider set. This is the
// existing ported Bionic implementation, not a raw Android libc image loader.
BionicProviderSet CreateBionicProviderSet(std::string* error);
}

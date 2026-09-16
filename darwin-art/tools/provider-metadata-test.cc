#include "../runtime/framework/pm/provider_metadata.h"
#include <cassert>
#include <iostream>

int main(int argc, char** argv) {
  using namespace darwin_art::framework::pm;
  assert(ValidateProviderMetadata(nullptr));
  assert(ValidateProviderMetadata("none"));
  assert(ValidateProviderMetadata("50>61>ffffffff>0>none"));
  assert(ValidateProviderMetadata("50>61>0>1>6b:s:,6c:i:ffffffff,6d:r:7f010001,6e:b:1"));
  const char* invalid[] = {
      "50>61>xyz>0>none", "50>61>100000000>0>none", "50>61>+1>0>none",
      "50>61>0>2>none", "50>61>0>true>none", "50>61>0>0>6b:i:bad!",
      "50>61>0>0>6b:b:2", "50>61>0>0>6b:x:1", "50>61>0>0>6b:s:f",
      "50>61>0>0>6b:s:00", "5>61>0>0>none", "50>>0>0>none",
      "50>61>0>0>6b", "50>61>0>0>none;"
  };
  for (const char* value : invalid) assert(!ValidateProviderMetadata(value));
  std::string decoded = "unchanged";
  assert(!DecodeHex("zz", &decoded) && decoded == "unchanged");
  assert(DecodeHex("4142", &decoded) && decoded == "AB");
  if (argc == 2) assert(ValidateProviderMetadata(argv[1]));
  std::cout << "provider metadata validation PASS\n";
}

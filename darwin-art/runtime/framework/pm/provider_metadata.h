#pragma once
#include <string>
#include <vector>

namespace darwin_art::framework::pm {
bool DecodeHex(const std::string& encoded, std::string* decoded);
std::vector<std::string> Split(const std::string& value, char delimiter);
// Pure package-record validation; never consults process-global environment.
bool ValidateProviderMetadata(const char* encoded);
}

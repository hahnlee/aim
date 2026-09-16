#pragma once
#include <string>
namespace darwin_art::filesystem {
// Read via the process guest descriptor table, including virtual regular files.
// No host pathname fallback; the guest descriptor is closed before returning.
bool ReadGuestConfig(const std::string& path, std::string* output);
// Restore Darwin errno after an Android filesystem ABI call fails.
void RestoreGuestConfigErrno();
}

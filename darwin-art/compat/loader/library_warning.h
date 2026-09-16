#pragma once
namespace darwin_art::loader {
// Producers must report the original object path and diagnostic separately.
// Process namespace owner must already be installed.
void AppendLinkerWarning(const char* path, const char* message, const char* value = nullptr);
}
extern "C" void __loader_android_dlwarning(void*, void (*)(void*, const char*));

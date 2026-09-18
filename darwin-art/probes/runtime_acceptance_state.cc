#include "runtime_acceptance_state.h"

#include <mutex>
#include <utility>

namespace darwin_art_acceptance {
namespace {

std::mutex g_mutex;
Snapshot g_snapshot;

}  // namespace

void record_network_elf_loaded() {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_snapshot.network_elf_loaded = true;
}

void record_apk_elf_loaded(std::string apk_sha256,
                           std::string apk_root_sha256) {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_snapshot.apk_elf_loaded = true;
  g_snapshot.apk_sha256 = std::move(apk_sha256);
  g_snapshot.apk_root_sha256 = std::move(apk_root_sha256);
}

void record_direct_apk_loaded() {
  std::lock_guard<std::mutex> lock(g_mutex);
  g_snapshot.direct_apk_loaded = true;
}

Snapshot snapshot() {
  std::lock_guard<std::mutex> lock(g_mutex);
  return g_snapshot;
}

}  // namespace darwin_art_acceptance

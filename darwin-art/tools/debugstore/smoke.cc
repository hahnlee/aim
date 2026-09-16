#include "debugstore/debugstore_cxx_bridge.rs.h"
#include <cassert>
#include <string>
#include <vector>
#include <thread>
#include <cstdio>

int main() {
  using namespace android::debugstore;
  const std::vector<std::string> attributes{"key", "value"};
  auto first = debug_store_begin("bind-application", attributes);
  auto second = debug_store_begin("second", {});
  assert(first != 0 && second != 0 && first != second);
  debug_store_end(first, attributes);
  debug_store_record("checkpoint", attributes);
  std::string text(debug_store_to_string());
  assert(text.find("bind-application") != std::string::npos);
  assert(text.find("checkpoint") != std::string::npos);
  std::vector<std::thread> writers;
  for (int i = 0; i < 4; ++i) writers.emplace_back([] {
    for (int j = 0; j < 100; ++j) {
      auto id = debug_store_begin("concurrent", {});
      assert(id != 0);
      debug_store_end(id, {});
    }
  });
  for (auto& writer : writers) writer.join();
  text = std::string(debug_store_to_string());
  assert(text.find("3,16,") == 0); // Original bounded storage, not unbounded logs.
  puts("AOSP DebugStore: begin/end/record IDs, snapshot, concurrent bounded storage PASS");
}

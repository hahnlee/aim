#include "compat/filesystem/document_panel.h"
#include <cassert>
#include <cstdio>
#include <thread>

int main() {
  // Wrong-thread calls must fail before constructing or showing any panel.
  // Interactive MIME/name/cancellation behavior is not asserted by this test.
  std::thread caller([] {
    assert(darwin_art_host_open_document(nullptr) == nullptr);
    assert(darwin_art_host_open_document("image/png") == nullptr);
    assert(darwin_art_host_save_document(nullptr, nullptr) == nullptr);
    assert(darwin_art_host_save_document("image/png", "image.png") == nullptr);
  });
  caller.join();
  std::puts("Document panel provider: off-AppKit-main admission rejection PASS");
}

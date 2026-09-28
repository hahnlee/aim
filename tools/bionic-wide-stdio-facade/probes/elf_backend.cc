#include "backend.h"

#include <errno.h>

#include <memory>
#include <vector>

namespace {

std::unique_ptr<wide_stdio_test::Context> g_context;
wide_stdio_test::Stream* g_input;
wide_stdio_test::Stream* g_output;
AimBionicWideStdioActivation* g_activation;

}  // namespace

extern "C" int wide_stdio_elf_backend_install() {
  if (g_context || g_activation) return -1;
  auto context = std::make_unique<wide_stdio_test::Context>();
  auto operations = wide_stdio_test::Operations(context.get());
  AimBionicWideStdioActivation* activation =
      aim_bionic_wide_stdio_install(&operations);
  if (activation == nullptr) return -1;
  g_input = context->Add({'A', 0xf0, 0x9f, 0x98, 0x80}, true, false);
  g_output = context->Add({}, false, true);
  g_activation = activation;
  g_context = std::move(context);
  return 0;
}

extern "C" AimAndroidFile* wide_stdio_elf_backend_input() {
  return g_input == nullptr ? nullptr : &g_input->token;
}

extern "C" AimAndroidFile* wide_stdio_elf_backend_output() {
  return g_output == nullptr ? nullptr : &g_output->token;
}

extern "C" int wide_stdio_elf_backend_verify() {
  if (g_output == nullptr || !g_output->error ||
      g_output->output !=
          std::vector<uint8_t>({0xf0, 0x9f, 0x98, 0x80, 0xed, 0xa0, 0x80})) {
    return -1;
  }
  return 0;
}

extern "C" int wide_stdio_elf_backend_uninstall() {
  if (g_context == nullptr || g_activation == nullptr) return -1;
  if (!g_context->Close(g_input) || !g_context->Close(g_output)) return -1;
  if (aim_bionic_wide_stdio_uninstall(&g_activation) != 0 ||
      g_activation != nullptr) {
    return -1;
  }
  g_input = nullptr;
  g_output = nullptr;
  g_context.reset();
  return 0;
}

#include "darwin_art_registered_native_bridge.h"

#include <cerrno>
#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

[[noreturn]] void Fail(const char* message) {
  std::fprintf(stderr, "registered-native-bridge-smoke: %s\n", message);
  std::exit(1);
}

void Require(bool condition, const char* message) {
  if (!condition) {
    Fail(message);
  }
}

uintptr_t ParseAddress(const char* text) {
  errno = 0;
  char* end = nullptr;
  const unsigned long long value = std::strtoull(text, &end, 0);
  if (errno != 0 || end == text || *end != '\0') {
    Fail("invalid address argument");
  }
  return static_cast<uintptr_t>(value);
}

bool ParseField(const char*& cursor, bool allow_void, char* short_type) {
  switch (*cursor) {
    case 'V':
      if (!allow_void) return false;
      *short_type = 'V';
      ++cursor;
      return true;
    case 'Z': case 'B': case 'C': case 'S': case 'I':
    case 'J': case 'F': case 'D':
      *short_type = *cursor++;
      return true;
    case 'L': {
      const char* terminator = std::strchr(cursor, ';');
      if (terminator == nullptr || terminator == cursor + 1) return false;
      cursor = terminator + 1;
      *short_type = 'L';
      return true;
    }
    case '[': {
      do {
        ++cursor;
      } while (*cursor == '[');
      if (*cursor == 'L') {
        const char* terminator = std::strchr(cursor, ';');
        if (terminator == nullptr || terminator == cursor + 1) return false;
        cursor = terminator + 1;
      } else if (std::strchr("ZBCSIJFD", *cursor) != nullptr) {
        ++cursor;
      } else {
        return false;
      }
      *short_type = 'L';
      return true;
    }
    default:
      return false;
  }
}

std::string DescriptorToShorty(const char* descriptor) {
  const char* cursor = descriptor;
  if (*cursor++ != '(') Fail("descriptor missing open parenthesis");
  std::string parameters;
  while (*cursor != ')') {
    char type = 0;
    if (*cursor == '\0' || !ParseField(cursor, false, &type)) {
      Fail("invalid descriptor parameter");
    }
    parameters.push_back(type);
  }
  ++cursor;
  char return_type = 0;
  if (!ParseField(cursor, true, &return_type) || *cursor != '\0') {
    Fail("invalid descriptor return");
  }
  return std::string(1, return_type) + parameters;
}

uint64_t Mix(uint64_t digest, uint64_t value) {
  return (digest ^ value) * UINT64_C(1099511628211);
}

int64_t AddThunkA(void*, void*, int32_t left, int64_t middle, int32_t right) {
  return static_cast<int64_t>(left) + middle + static_cast<int64_t>(right);
}

int64_t AddThunkB(void*, void*, int32_t left, int64_t middle, int32_t right) {
  return static_cast<int64_t>(left) + middle + static_cast<int64_t>(right);
}

int64_t CriticalAddThunk(int32_t left, int64_t middle, int32_t right) {
  return static_cast<int64_t>(left) + middle + static_cast<int64_t>(right);
}

int64_t SpillThunk(void*,
                   void*,
                   uint8_t z,
                   int8_t b,
                   uint16_t c,
                   int16_t s,
                   int32_t i,
                   int64_t j,
                   void* reference,
                   float f0,
                   double d0,
                   float f1,
                   double d1,
                   float f2,
                   double d2,
                   float f3,
                   double d3,
                   float f4,
                   float f5,
                   double d4) {
  const float floats[] = {f0, f1, f2, f3, f4, f5};
  const double doubles[] = {d0, d1, d2, d3, d4};
  uint64_t digest = UINT64_C(1469598103934665603);
  digest = Mix(digest, z);
  digest = Mix(digest, static_cast<uint8_t>(b));
  digest = Mix(digest, c);
  digest = Mix(digest, static_cast<uint16_t>(s));
  digest = Mix(digest, static_cast<uint32_t>(i));
  digest = Mix(digest, static_cast<uint64_t>(j));
  digest = Mix(digest, reference != nullptr);
  for (size_t index = 0; index < 5; ++index) {
    uint32_t float_bits = 0;
    uint64_t double_bits = 0;
    std::memcpy(&float_bits, &floats[index], sizeof(float_bits));
    std::memcpy(&double_bits, &doubles[index], sizeof(double_bits));
    digest = Mix(digest, float_bits);
    digest = Mix(digest, double_bits);
  }
  uint32_t final_float_bits = 0;
  std::memcpy(&final_float_bits, &floats[5], sizeof(final_float_bits));
  digest = Mix(digest, final_float_bits);
  return static_cast<int64_t>(digest);
}

int64_t DarwinDirect(int64_t value) {
  return value + 1;
}

struct FakeContext {
  DarwinArtAndroidFunctionOwnerV1 owner;
  uintptr_t add;
  uintptr_t spill;
  bool active;
  size_t owner_leases;
  size_t builds;
  size_t destroys;
};

int LookupOwner(void* opaque,
                const void* function,
                DarwinArtAndroidFunctionOwnerV1* owner_out) {
  auto* context = static_cast<FakeContext*>(opaque);
  const uintptr_t address = reinterpret_cast<uintptr_t>(function);
  if (!context->active || address < context->owner.executable_begin ||
      address >= context->owner.executable_end) {
    return 0;
  }
  *owner_out = context->owner;
  ++context->owner_leases;
  return 1;
}

void ReleaseOwner(void* opaque, const DarwinArtAndroidFunctionOwnerV1* owner) {
  auto* context = static_cast<FakeContext*>(opaque);
  Require(owner->image_id == context->owner.image_id &&
              owner->generation == context->owner.generation &&
              context->owner_leases != 0,
          "owner lease release mismatch");
  --context->owner_leases;
}

void* BuildThunk(void* opaque,
                 const void* android_function,
                 const DarwinArtAndroidFunctionOwnerV1* owner,
                 const char* shorty,
                 uint32_t shorty_length,
                 DarwinArtJniCallType call_type) {
  auto* context = static_cast<FakeContext*>(opaque);
  const uintptr_t address = reinterpret_cast<uintptr_t>(android_function);
  const std::string key(shorty, shorty_length);
  ++context->builds;
  if (address == context->add && key == "JIJI") {
    if (call_type == DARWIN_ART_JNI_CALL_CRITICAL_NATIVE) {
      return reinterpret_cast<void*>(&CriticalAddThunk);
    }
    return owner->generation == 1 ? reinterpret_cast<void*>(&AddThunkA)
                                  : reinterpret_cast<void*>(&AddThunkB);
  }
  if (address == context->spill && key == "JZBCSIJLFDFDFDFDFFD" &&
      call_type == DARWIN_ART_JNI_CALL_REGULAR) {
    return reinterpret_cast<void*>(&SpillThunk);
  }
  return nullptr;
}

void DestroyThunk(void* opaque, void*) {
  auto* context = static_cast<FakeContext*>(opaque);
  ++context->destroys;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc != 5) {
    Fail("usage: smoke EXEC_BEGIN EXEC_END NATIVE_ADD NATIVE_SPILL");
  }
  const uintptr_t executable_begin = ParseAddress(argv[1]);
  const uintptr_t executable_end = ParseAddress(argv[2]);
  const uintptr_t native_add = ParseAddress(argv[3]);
  const uintptr_t native_spill = ParseAddress(argv[4]);
  Require(executable_begin < native_add && native_add < native_spill &&
              native_spill < executable_end,
          "fixture symbols are outside executable ownership range");

  const std::string add_shorty = DescriptorToShorty("(IJI)J");
  const std::string spill_shorty = DescriptorToShorty(
      "(ZBCSIJLjava/lang/Object;FDFDFDFDFFD)J");
  Require(add_shorty == "JIJI", "nativeAdd shorty mismatch");
  Require(spill_shorty == "JZBCSIJLFDFDFDFDFFD",
          "nativeSpill shorty mismatch");

  FakeContext context{{UINT64_C(0x415254454c46),
                       1,
                       executable_begin,
                       executable_end},
                      native_add,
                      native_spill,
                      true,
                      0,
                      0,
                      0};
  const DarwinArtRegisteredNativeThunkFactoryV1 factory{
      DARWIN_ART_REGISTERED_NATIVE_BRIDGE_ABI_VERSION,
      sizeof(DarwinArtRegisteredNativeThunkFactoryV1),
      &context,
      &LookupOwner,
      &ReleaseOwner,
      &BuildThunk,
      &DestroyThunk};
  DarwinArtRegisteredNativeCache* cache =
      darwin_art_registered_native_cache_create(&factory);
  Require(cache != nullptr, "cache create failed");

  const void* callable = nullptr;
  auto resolution = darwin_art_resolve_registered_native(
      cache,
      reinterpret_cast<const void*>(native_add),
      false,
      add_shorty.c_str(),
      static_cast<uint32_t>(add_shorty.size()),
      DARWIN_ART_JNI_CALL_REGULAR,
      &callable);
  Require(resolution == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE,
          "owned pointer did not select bridge");
  void* first_add = const_cast<void*>(callable);
  using AddFunction = int64_t (*)(void*, void*, int32_t, int64_t, int32_t);
  Require(reinterpret_cast<AddFunction>(first_add)(nullptr, nullptr, 7, 100, -2) ==
              105,
          "Darwin-callable nativeAdd thunk failed");

  const void* repeated = nullptr;
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(native_add),
              true,
              add_shorty.c_str(),
              static_cast<uint32_t>(add_shorty.size()),
              DARWIN_ART_JNI_CALL_REGULAR,
              &repeated) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE &&
              repeated == callable && context.builds == 1,
          "same registration key did not reuse cached thunk");

  const void* spill_callable = nullptr;
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(native_spill),
              false,
              spill_shorty.c_str(),
              static_cast<uint32_t>(spill_shorty.size()),
              DARWIN_ART_JNI_CALL_REGULAR,
              &spill_callable) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE,
          "mixed spill registration failed");
  using SpillFunction = int64_t (*)(void*, void*, uint8_t, int8_t, uint16_t,
                                    int16_t, int32_t, int64_t, void*, float,
                                    double, float, double, float, double, float,
                                    double, float, float, double);
  const uint64_t spill_digest = static_cast<uint64_t>(
      reinterpret_cast<SpillFunction>(const_cast<void*>(spill_callable))(
          nullptr, nullptr, 1, -2, 0x4567, -1234, 0x10203040,
          INT64_C(0x1122334455667788), reinterpret_cast<void*>(0x1234),
          1.25f, -2.5, 3.75f, 4.5, -5.25f, 6.75, 7.125f, -8.875,
          9.5f, -11.75f, 10.25));

  const void* critical_callable = nullptr;
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(native_add),
              true,
              add_shorty.c_str(),
              static_cast<uint32_t>(add_shorty.size()),
              DARWIN_ART_JNI_CALL_CRITICAL_NATIVE,
              &critical_callable) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE &&
              critical_callable != first_add,
          "call type was not part of trampoline key");
  using CriticalAddFunction = int64_t (*)(int32_t, int64_t, int32_t);
  Require(reinterpret_cast<CriticalAddFunction>(
              const_cast<void*>(critical_callable))(7, 100, -2) == 105,
          "critical-native implicit-argument shape failed");

  const void* direct_callable = nullptr;
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(&DarwinDirect),
              false,
              "JJ",
              2,
              DARWIN_ART_JNI_CALL_REGULAR,
              &direct_callable) == DARWIN_ART_REGISTERED_NATIVE_DIRECT &&
              direct_callable == reinterpret_cast<const void*>(&DarwinDirect),
          "unbridged Darwin pointer was not preserved");
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(&DarwinDirect),
              true,
              "JJ",
              2,
              DARWIN_ART_JNI_CALL_REGULAR,
              &direct_callable) == DARWIN_ART_REGISTERED_NATIVE_ERROR,
          "bridged namespace accepted a foreign function pointer");
  Require(darwin_art_get_registered_native_trampoline(
              cache,
              reinterpret_cast<const void*>(native_add),
              "JIJI",
              3,
              DARWIN_ART_JNI_CALL_REGULAR) == nullptr,
          "shorty length mismatch was accepted");
  Require(darwin_art_get_registered_native_trampoline(
              cache,
              reinterpret_cast<const void*>(native_spill),
              spill_shorty.c_str(),
              static_cast<uint32_t>(spill_shorty.size()),
              DARWIN_ART_JNI_CALL_CRITICAL_NATIVE) == nullptr,
          "critical native reference parameter was accepted");
  Require(darwin_art_registered_native_cache_size(cache) == 3,
          "unexpected first-generation cache size");

  context.active = false;
  Require(context.owner_leases == 3, "cached entries lost their image leases");
  const size_t retired = darwin_art_registered_native_cache_retire_image(
      cache, context.owner.image_id, context.owner.generation);
  Require(retired == 3 && context.destroys == 3 && context.owner_leases == 0 &&
              darwin_art_registered_native_cache_size(cache) == 0,
          "image retirement did not release exact generation");
  Require(!darwin_art_is_android_function_pointer(
              cache, reinterpret_cast<const void*>(native_add)),
          "closed image still owns function address");

  context.owner.generation = 2;
  context.active = true;
  const void* reused_address_callable = nullptr;
  Require(darwin_art_resolve_registered_native(
              cache,
              reinterpret_cast<const void*>(native_add),
              true,
              add_shorty.c_str(),
              static_cast<uint32_t>(add_shorty.size()),
              DARWIN_ART_JNI_CALL_REGULAR,
              &reused_address_callable) == DARWIN_ART_REGISTERED_NATIVE_TRAMPOLINE &&
              reused_address_callable != first_add,
          "address reuse resurrected previous image generation thunk");

  std::printf("fixture-shorty add=%s spill=%s\n",
              add_shorty.c_str(), spill_shorty.c_str());
  std::printf("ownership=elf-exec-range bridge-or=namespace|pointer cache-key=image+generation+address+shorty+call-type\n");
  std::printf("callable add=105 spill=%016" PRIx64 " regular=1 critical=2\n",
              spill_digest);
  std::printf("lifecycle=retire-on-image-close retired=%zu generation-reuse=fresh\n",
              retired);

  darwin_art_registered_native_cache_destroy(cache);
  Require(context.builds == 4 && context.destroys == 4 && context.owner_leases == 0,
          "cache destroy/build lifecycle mismatch");
  return 0;
}

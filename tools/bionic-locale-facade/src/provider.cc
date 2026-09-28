#include "aim_bionic_locale.h"

#include <errno.h>
#include <fenv.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <climits>
#include <cstdlib>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <new>
#include <string_view>
#include <unordered_map>

#include <androidicuinit/android_icu_init.h>
#include <unicode/uchar.h>
#include <unicode/uversion.h>

extern "C" void aim_bionic_errno_store(int32_t android_errno);
extern bool android_icu_is_registered();

namespace {

static_assert(sizeof(AimAndroidMbState) == 8);
static_assert(alignof(AimAndroidMbState) == 1);
static_assert(sizeof(uint32_t) == 4);
static_assert(sizeof(AimAndroidLconv) == 96);

constexpr int kAndroidEnoent = 2;
constexpr int kAndroidEnomem = 12;
constexpr int kAndroidEinval = 22;
constexpr int kAndroidEilseq = 84;
constexpr int kAndroidLcCtype = 0;
constexpr int kAndroidLcIdentification = 12;
constexpr int kAndroidLcAllMask = 0x1fbf;
constexpr size_t kIllegal = static_cast<size_t>(-1);
constexpr size_t kIncomplete = static_cast<size_t>(-2);
constexpr uint32_t kAndroidWeof = UINT32_MAX;
constexpr int kEof = -1;

constexpr char kCtypeUpper = 0x01;
constexpr char kCtypeLower = 0x02;
constexpr char kCtypeDigit = 0x04;
constexpr char kCtypeSpace = 0x08;
constexpr char kCtypePunct = 0x10;
constexpr char kCtypeControl = 0x20;
constexpr char kCtypeHex = 0x40;
constexpr char kCtypeBlank = static_cast<char>(0x80);

constexpr std::array<char, 257> MakeAndroidCtypeTable() {
  std::array<char, 257> result{};
  for (int value = 0; value < 32; ++value) {
    result[static_cast<size_t>(value) + 1] = kCtypeControl;
  }
  for (int value = 9; value <= 13; ++value) {
    result[static_cast<size_t>(value) + 1] |= kCtypeSpace;
  }
  result[static_cast<size_t>(' ') + 1] = kCtypeSpace | kCtypeBlank;
  for (int value = '!'; value <= '~'; ++value) {
    result[static_cast<size_t>(value) + 1] = kCtypePunct;
  }
  for (int value = '0'; value <= '9'; ++value) {
    result[static_cast<size_t>(value) + 1] = kCtypeDigit;
  }
  for (int value = 'A'; value <= 'Z'; ++value) {
    result[static_cast<size_t>(value) + 1] =
        kCtypeUpper | (value <= 'F' ? kCtypeHex : 0);
  }
  for (int value = 'a'; value <= 'z'; ++value) {
    result[static_cast<size_t>(value) + 1] =
        kCtypeLower | (value <= 'f' ? kCtypeHex : 0);
  }
  result[128] = kCtypeControl;
  for (size_t value = 128; value < 160; ++value) {
    result[value + 1] = kCtypeControl;
  }
  for (size_t value = 160; value < 256; ++value) {
    result[value + 1] = kCtypePunct;
  }
  return result;
}

constexpr auto kAndroidCtypeTable = MakeAndroidCtypeTable();

static_assert(U_ICU_VERSION_MAJOR_NUM == 76);
static_assert(U_ICU_VERSION_MINOR_NUM == 1);

struct HostStateGuard {
  int saved_errno{errno};
  fenv_t saved_environment{};
  HostStateGuard() { fegetenv(&saved_environment); }
  ~HostStateGuard() {
    fesetenv(&saved_environment);
    errno = saved_errno;
  }
};

enum class LocaleMode { kC, kUtf8 };

struct LocaleEntry {
  explicit LocaleEntry(LocaleMode mode_value) : mode(mode_value) {}
  LocaleMode mode;
};

struct ProviderState {
  std::mutex mutex;
  std::unordered_map<AimAndroidLocale, std::unique_ptr<LocaleEntry>>
      locales;
  std::atomic<bool> global_utf8{true};
};

ProviderState& State() {
  static ProviderState state;
  return state;
}

struct ThreadLocaleState {
  AimAndroidLocale handle{};
  LocaleMode mode{LocaleMode::kUtf8};
};

thread_local ThreadLocaleState g_thread_locale;
thread_local AimAndroidMbState g_mbrtowc_state{};
thread_local AimAndroidMbState g_mbrlen_state{};
thread_local AimAndroidMbState g_mbsnrtowcs_state{};
thread_local AimAndroidMbState g_mbtowc_state{};
thread_local AimAndroidMbState g_wcrtomb_state{};
thread_local AimAndroidMbState g_wcsnrtombs_state{};

AimAndroidLocale GlobalLocale() {
  return reinterpret_cast<AimAndroidLocale>(UINTPTR_MAX);
}

void SetAndroidErrno(int value) { aim_bionic_errno_store(value); }

void EnsureAndroidIcu76() {
  static std::once_flag once;
  std::call_once(once, [] {
    if (!android_icu_is_registered()) android_icu_init();
    if (!android_icu_is_registered()) std::abort();
    UVersionInfo version{};
    u_getVersion(version);
    if (version[0] != 76 || version[1] != 1) std::abort();
  });
}

UChar32 IcuCodePoint(uint32_t code_point) {
  return static_cast<UChar32>(code_point);
}

bool IsSupportedLocale(const char* name) {
  return std::strcmp(name, "") == 0 || std::strcmp(name, "C") == 0 ||
         std::strcmp(name, "C.UTF-8") == 0 ||
         std::strcmp(name, "en_US.UTF-8") == 0 ||
         std::strcmp(name, "POSIX") == 0;
}

LocaleMode ModeForName(const char* name) {
  return (*name == '\0' || std::strstr(name, "UTF-8") != nullptr)
             ? LocaleMode::kUtf8
             : LocaleMode::kC;
}

bool StateInitial(const AimAndroidMbState* state) {
  uint32_t value = 0;
  std::memcpy(&value, state->sequence, sizeof(value));
  return value == 0;
}

size_t StateBytes(const AimAndroidMbState* state) {
  return state->sequence[2] != 0 ? 3
                                : state->sequence[1] != 0
                                      ? 2
                                      : state->sequence[0] != 0 ? 1 : 0;
}

void ResetState(AimAndroidMbState* state) {
  std::memset(state->sequence, 0, sizeof(state->sequence));
}

size_t Illegal(int android_errno, AimAndroidMbState* state) {
  SetAndroidErrno(android_errno);
  ResetState(state);
  return kIllegal;
}

size_t Decode(uint32_t* output,
              const char* source,
              size_t length,
              AimAndroidMbState* state) {
  if (state->sequence[3] != 0) return Illegal(kAndroidEinval, state);
  static constexpr char kEmpty[] = "";
  if (source == nullptr) {
    source = kEmpty;
    length = 1;
    output = nullptr;
  }
  if (length == 0) return kIncomplete;

  uint8_t byte = static_cast<uint8_t>(*source);
  if (StateInitial(state) && byte < 0x80) {
    if (output != nullptr) *output = byte;
    return byte == 0 ? 0 : 1;
  }

  const size_t bytes_so_far = StateBytes(state);
  byte = bytes_so_far == 0 ? static_cast<uint8_t>(*source)
                           : state->sequence[0];
  size_t sequence_length = 0;
  uint32_t mask = 0;
  uint32_t lower_bound = 0;
  if ((byte & 0xe0) == 0xc0) {
    sequence_length = 2;
    mask = 0x1f;
    lower_bound = 0x80;
  } else if ((byte & 0xf0) == 0xe0) {
    sequence_length = 3;
    mask = 0x0f;
    lower_bound = 0x800;
  } else if ((byte & 0xf8) == 0xf0) {
    sequence_length = 4;
    mask = 0x07;
    lower_bound = 0x10000;
  } else {
    return Illegal(kAndroidEilseq, state);
  }

  const size_t wanted = sequence_length - bytes_so_far;
  size_t consumed = 0;
  for (; consumed < std::min(wanted, length); ++consumed) {
    const uint8_t current = static_cast<uint8_t>(*source++);
    if (!StateInitial(state) && (current & 0xc0) != 0x80) {
      return Illegal(kAndroidEilseq, state);
    }
    state->sequence[bytes_so_far + consumed] = current;
  }
  if (consumed < wanted) return kIncomplete;

  uint32_t code_point = state->sequence[0] & mask;
  for (size_t index = 1; index < sequence_length; ++index) {
    code_point = (code_point << 6) | (state->sequence[index] & 0x3f);
  }
  if (code_point < lower_bound ||
      (code_point >= 0xd800 && code_point <= 0xdfff) ||
      code_point > 0x10ffff) {
    return Illegal(kAndroidEilseq, state);
  }
  if (output != nullptr) *output = code_point;
  ResetState(state);
  return code_point == 0 ? 0 : wanted;
}

size_t Encode(char* destination,
              uint32_t code_point,
              AimAndroidMbState* state) {
  if (destination == nullptr) {
    ResetState(state);
    return 1;
  }
  if (code_point == 0) {
    *destination = '\0';
    ResetState(state);
    return 1;
  }
  if (!StateInitial(state)) return Illegal(kAndroidEilseq, state);
  if (code_point < 0x80) {
    *destination = static_cast<char>(code_point);
    return 1;
  }
  uint8_t lead = 0;
  size_t length = 0;
  if (code_point <= 0x7ff) {
    lead = 0xc0;
    length = 2;
  } else if (code_point <= 0xffff) {
    lead = 0xe0;
    length = 3;
  } else if (code_point <= 0x1fffff) {
    lead = 0xf0;
    length = 4;
  } else {
    SetAndroidErrno(kAndroidEilseq);
    return kIllegal;
  }
  for (size_t index = length - 1; index > 0; --index) {
    destination[index] = static_cast<char>((code_point & 0x3f) | 0x80);
    code_point >>= 6;
  }
  destination[0] = static_cast<char>((code_point & 0xff) | lead);
  return length;
}

size_t WideLength(const uint32_t* source) {
  const uint32_t* cursor = source;
  while (*cursor != 0) ++cursor;
  return static_cast<size_t>(cursor - source);
}

int WideCompare(const uint32_t* left, const uint32_t* right) {
  while (*left == *right && *left != 0) {
    ++left;
    ++right;
  }
  return *left < *right ? -1 : *left > *right ? 1 : 0;
}

char g_decimal_point[] = ".";
char g_empty[] = "";
char g_c_locale[] = "C";
char g_utf8_locale[] = "C.UTF-8";
constexpr char kAndroidCharMax = static_cast<char>(UINT8_MAX);
AimAndroidLconv g_localeconv = {
    g_decimal_point, g_empty, g_empty, g_empty, g_empty,
    g_empty,         g_empty, g_empty, g_empty, g_empty,
    kAndroidCharMax,  kAndroidCharMax, kAndroidCharMax, kAndroidCharMax,
    kAndroidCharMax,  kAndroidCharMax, kAndroidCharMax, kAndroidCharMax,
    kAndroidCharMax,  kAndroidCharMax, kAndroidCharMax, kAndroidCharMax,
    kAndroidCharMax,  kAndroidCharMax};

}  // namespace

extern "C" const char* aim_bionic__ctype_ =
    kAndroidCtypeTable.data();

extern "C" size_t aim_bionic___ctype_get_mb_cur_max(void) {
  HostStateGuard guard;
  const LocaleMode mode =
      g_thread_locale.handle == nullptr
          ? (State().global_utf8.load(std::memory_order_acquire)
                 ? LocaleMode::kUtf8
                 : LocaleMode::kC)
          : g_thread_locale.mode;
  return mode == LocaleMode::kUtf8 ? 4 : 1;
}

extern "C" AimAndroidLocale aim_bionic_newlocale(
    int category_mask,
    const char* locale_name,
    AimAndroidLocale /*base*/) {
  HostStateGuard guard;
  if ((category_mask & ~kAndroidLcAllMask) != 0 || locale_name == nullptr) {
    SetAndroidErrno(kAndroidEinval);
    return nullptr;
  }
  if (!IsSupportedLocale(locale_name)) {
    SetAndroidErrno(kAndroidEnoent);
    return nullptr;
  }
  std::unique_ptr<LocaleEntry> entry(
      new (std::nothrow) LocaleEntry(ModeForName(locale_name)));
  if (entry == nullptr) {
    SetAndroidErrno(kAndroidEnomem);
    return nullptr;
  }
  AimAndroidLocale handle = entry.get();
  try {
    ProviderState& state = State();
    std::lock_guard<std::mutex> lock(state.mutex);
    state.locales.emplace(handle, std::move(entry));
  } catch (...) {
    SetAndroidErrno(kAndroidEnomem);
    return nullptr;
  }
  return handle;
}

extern "C" void aim_bionic_freelocale(
    AimAndroidLocale locale) {
  HostStateGuard guard;
  if (locale == nullptr || locale == GlobalLocale()) return;
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.mutex);
  state.locales.erase(locale);
}

extern "C" int aim_bionic_iswalpha_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_ALPHABETIC);
}

extern "C" int aim_bionic_iswalpha(uint32_t code_point) {
  return aim_bionic_iswalpha_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_isdigit_l(
    int value, AimAndroidLocale /*locale*/) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 && (kAndroidCtypeTable[code + 1] & kCtypeDigit) != 0;
}

extern "C" int aim_bionic_isalnum(int value) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 &&
         (kAndroidCtypeTable[code + 1] &
          (kCtypeUpper | kCtypeLower | kCtypeDigit)) != 0;
}

extern "C" int aim_bionic_isalpha(int value) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 &&
         (kAndroidCtypeTable[code + 1] & (kCtypeUpper | kCtypeLower)) != 0;
}

extern "C" int aim_bionic_islower_l(
    int value, AimAndroidLocale /*locale*/) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 && (kAndroidCtypeTable[code + 1] & kCtypeLower) != 0;
}

extern "C" int aim_bionic_islower(int value) {
  return aim_bionic_islower_l(value, GlobalLocale());
}

extern "C" int aim_bionic_isupper_l(
    int value, AimAndroidLocale /*locale*/) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 && (kAndroidCtypeTable[code + 1] & kCtypeUpper) != 0;
}

extern "C" int aim_bionic_isupper(int value) {
  return aim_bionic_isupper_l(value, GlobalLocale());
}

extern "C" int aim_bionic_isxdigit_l(
    int value, AimAndroidLocale /*locale*/) {
  const unsigned code = static_cast<unsigned>(value);
  return code < 256 && (kAndroidCtypeTable[code + 1] &
                         (kCtypeDigit | kCtypeHex)) != 0;
}

extern "C" int aim_bionic_isxdigit(int value) {
  return aim_bionic_isxdigit_l(value, GlobalLocale());
}

extern "C" int aim_bionic_iswblank_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_POSIX_BLANK);
}

extern "C" int aim_bionic_iswblank(uint32_t code_point) {
  return aim_bionic_iswblank_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswcntrl_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_charType(IcuCodePoint(code_point)) == U_CONTROL_CHAR;
}

extern "C" int aim_bionic_iswcntrl(uint32_t code_point) {
  return aim_bionic_iswcntrl_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswdigit_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_isdigit(IcuCodePoint(code_point));
}

extern "C" int aim_bionic_iswdigit(uint32_t code_point) {
  return aim_bionic_iswdigit_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswlower_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_LOWERCASE);
}

extern "C" int aim_bionic_iswlower(uint32_t code_point) {
  return aim_bionic_iswlower_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswprint_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_POSIX_PRINT);
}

extern "C" int aim_bionic_iswprint(uint32_t code_point) {
  return aim_bionic_iswprint_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswpunct_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_ispunct(IcuCodePoint(code_point));
}

extern "C" int aim_bionic_iswpunct(uint32_t code_point) {
  return aim_bionic_iswpunct_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswspace_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_WHITE_SPACE);
}

extern "C" int aim_bionic_iswspace(uint32_t code_point) {
  return aim_bionic_iswspace_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswupper_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_UPPERCASE);
}

extern "C" int aim_bionic_iswupper(uint32_t code_point) {
  return aim_bionic_iswupper_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_iswxdigit_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  return u_hasBinaryProperty(IcuCodePoint(code_point), UCHAR_POSIX_XDIGIT);
}

extern "C" int aim_bionic_iswxdigit(uint32_t code_point) {
  return aim_bionic_iswxdigit_l(code_point, GlobalLocale());
}

extern "C" AimAndroidLocale aim_bionic_uselocale(
    AimAndroidLocale locale) {
  HostStateGuard guard;
  AimAndroidLocale old =
      g_thread_locale.handle == nullptr ? GlobalLocale()
                                        : g_thread_locale.handle;
  if (locale == nullptr) return old;
  if (locale == GlobalLocale()) {
    g_thread_locale.handle = nullptr;
    return old;
  }
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.mutex);
  const auto found = state.locales.find(locale);
  if (found == state.locales.end()) {
    SetAndroidErrno(kAndroidEinval);
    return nullptr;
  }
  g_thread_locale.handle = locale;
  g_thread_locale.mode = found->second->mode;
  return old;
}

extern "C" char* aim_bionic_setlocale(int category,
                                               const char* locale_name) {
  HostStateGuard guard;
  if (category < kAndroidLcCtype || category > kAndroidLcIdentification) {
    SetAndroidErrno(kAndroidEinval);
    return nullptr;
  }
  ProviderState& state = State();
  if (locale_name != nullptr) {
    if (!IsSupportedLocale(locale_name)) {
      SetAndroidErrno(kAndroidEnoent);
      return nullptr;
    }
    state.global_utf8.store(ModeForName(locale_name) == LocaleMode::kUtf8,
                            std::memory_order_release);
  }
  return state.global_utf8.load(std::memory_order_acquire) ? g_utf8_locale
                                                           : g_c_locale;
}

extern "C" AimAndroidLconv* aim_bionic_localeconv(void) {
  HostStateGuard guard;
  return &g_localeconv;
}

extern "C" size_t aim_bionic_mbrtowc(
    uint32_t* output,
    const char* source,
    size_t length,
    AimAndroidMbState* state) {
  HostStateGuard guard;
  return Decode(output, source, length,
                state == nullptr ? &g_mbrtowc_state : state);
}

extern "C" size_t aim_bionic_mbrlen(
    const char* source,
    size_t length,
    AimAndroidMbState* state) {
  HostStateGuard guard;
  return Decode(nullptr, source, length,
                state == nullptr ? &g_mbrlen_state : state);
}

extern "C" int aim_bionic_mbtowc(uint32_t* output,
                                         const char* source,
                                         size_t length) {
  HostStateGuard guard;
  if (source == nullptr) {
    ResetState(&g_mbtowc_state);
    return 0;
  }
  const size_t result = Decode(output, source, length, &g_mbtowc_state);
  if (result == kIncomplete) SetAndroidErrno(kAndroidEilseq);
  return result == kIllegal || result == kIncomplete
             ? -1
             : static_cast<int>(result);
}

extern "C" uint32_t aim_bionic_btowc(int byte) {
  HostStateGuard guard;
  if (byte == kEof) return kAndroidWeof;
  AimAndroidMbState state{};
  const char character = static_cast<char>(byte);
  uint32_t output = 0;
  const size_t result = Decode(&output, &character, 1, &state);
  return result > 1 ? kAndroidWeof : output;
}

extern "C" size_t aim_bionic_mbsnrtowcs(
    uint32_t* destination,
    const char** source,
    size_t source_length,
    size_t destination_length,
    AimAndroidMbState* state_pointer) {
  HostStateGuard guard;
  AimAndroidMbState* state =
      state_pointer == nullptr ? &g_mbsnrtowcs_state : state_pointer;
  if (source == nullptr || *source == nullptr) return 0;
  if (source_length > 0 && StateBytes(state) > 0 &&
      static_cast<uint8_t>((*source)[0]) < 0x80) {
    return Illegal(kAndroidEilseq, state);
  }
  size_t input = 0;
  size_t output = 0;
  if (destination == nullptr) {
    while (input < source_length) {
      size_t consumed = 1;
      if (static_cast<uint8_t>((*source)[input]) < 0x80) {
        if ((*source)[input] == '\0') {
          ResetState(state);
          return output;
        }
      } else {
        consumed = Decode(nullptr, *source + input, source_length - input,
                          state);
        if (consumed == kIllegal || consumed == kIncomplete) {
          return Illegal(kAndroidEilseq, state);
        }
        if (consumed == 0) {
          ResetState(state);
          return output;
        }
      }
      input += consumed;
      ++output;
    }
    ResetState(state);
    return output;
  }
  while (input < source_length && output < destination_length) {
    size_t consumed = 1;
    if (static_cast<uint8_t>((*source)[input]) < 0x80) {
      destination[output] = static_cast<uint8_t>((*source)[input]);
      if ((*source)[input] == '\0') {
        *source = nullptr;
        ResetState(state);
        return output;
      }
    } else {
      consumed = Decode(destination + output, *source + input,
                        source_length - input, state);
      if (consumed == kIllegal) {
        *source += input;
        return Illegal(kAndroidEilseq, state);
      }
      if (consumed == kIncomplete) {
        *source += source_length;
        return Illegal(kAndroidEilseq, state);
      }
      if (consumed == 0) {
        *source = nullptr;
        ResetState(state);
        return output;
      }
    }
    input += consumed;
    ++output;
  }
  *source += input;
  ResetState(state);
  return output;
}

extern "C" size_t aim_bionic_mbsrtowcs(
    uint32_t* destination,
    const char** source,
    size_t destination_length,
    AimAndroidMbState* state) {
  return aim_bionic_mbsnrtowcs(destination, source, SIZE_MAX,
                                      destination_length, state);
}

extern "C" size_t aim_bionic_wcrtomb(
    char* destination,
    uint32_t code_point,
    AimAndroidMbState* state) {
  HostStateGuard guard;
  return Encode(destination, code_point,
                state == nullptr ? &g_wcrtomb_state : state);
}

extern "C" size_t aim_bionic_wcsnrtombs(
    char* destination,
    const uint32_t** source,
    size_t source_length,
    size_t destination_length,
    AimAndroidMbState* state_pointer) {
  HostStateGuard guard;
  AimAndroidMbState* state =
      state_pointer == nullptr ? &g_wcsnrtombs_state : state_pointer;
  if (source == nullptr || *source == nullptr) return 0;
  if (!StateInitial(state)) return Illegal(kAndroidEilseq, state);
  char buffer[4];
  size_t input = 0;
  size_t output = 0;
  if (destination == nullptr) {
    while (input < source_length) {
      const uint32_t code_point = (*source)[input++];
      if (code_point == 0) return output;
      const size_t encoded = Encode(buffer, code_point, state);
      if (encoded == kIllegal) return encoded;
      output += encoded;
    }
    return output;
  }
  while (input < source_length && output < destination_length) {
    const uint32_t code_point = (*source)[input];
    if (code_point == 0) {
      destination[output] = '\0';
      *source = nullptr;
      return output;
    }
    const size_t encoded = Encode(buffer, code_point, state);
    if (encoded == kIllegal) {
      *source += input;
      return encoded;
    }
    if (encoded > destination_length - output) break;
    std::memcpy(destination + output, buffer, encoded);
    output += encoded;
    ++input;
  }
  *source += input;
  return output;
}

extern "C" int aim_bionic_wctob(uint32_t code_point) {
  HostStateGuard guard;
  if (code_point == kAndroidWeof) return kEof;
  AimAndroidMbState state{};
  char buffer[4];
  const size_t result = Encode(buffer, code_point, &state);
  return result == 1 ? static_cast<unsigned char>(buffer[0]) : kEof;
}

extern "C" int aim_bionic_strcoll_l(
    const char* left,
    const char* right,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  return std::strcmp(left, right);
}

extern "C" int aim_bionic_strcoll(const char* left,
                                            const char* right) {
  return aim_bionic_strcoll_l(left, right, GlobalLocale());
}

extern "C" size_t aim_bionic_strxfrm_l(
    char* destination,
    const char* source,
    size_t length,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  const size_t source_length = std::strlen(source);
  if (length != 0) {
    const size_t copy = std::min(source_length, length - 1);
    std::memcpy(destination, source, copy);
    destination[copy] = '\0';
  }
  return source_length;
}

extern "C" size_t aim_bionic_strxfrm(char* destination,
                                             const char* source,
                                             size_t length) {
  return aim_bionic_strxfrm_l(destination, source, length,
                                     GlobalLocale());
}

extern "C" int aim_bionic_tolower(int value) {
  return value >= 'A' && value <= 'Z' ? value | 0x20 : value;
}

extern "C" int aim_bionic_toupper(int value) {
  return value >= 'a' && value <= 'z' ? value ^ 0x20 : value;
}

extern "C" int aim_bionic_tolower_l(
    int value,
    AimAndroidLocale /*locale*/) {
  // Android's locale_t is an opaque Bionic handle.  The supported locale
  // set has C byte semantics, so do not pass that guest handle to Darwin.
  return aim_bionic_tolower(value);
}

extern "C" int aim_bionic_toupper_l(
    int value,
    AimAndroidLocale /*locale*/) {
  return aim_bionic_toupper(value);
}

extern "C" uint32_t aim_bionic_towlower_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  if (code_point < 0x80) {
    return code_point >= 'A' && code_point <= 'Z' ? code_point | 0x20
                                                  : code_point;
  }
  EnsureAndroidIcu76();
  return static_cast<uint32_t>(u_tolower(IcuCodePoint(code_point)));
}

extern "C" uint32_t aim_bionic_towlower(uint32_t code_point) {
  return aim_bionic_towlower_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_wcwidth(uint32_t code_point) {
  HostStateGuard guard;
  EnsureAndroidIcu76();
  const UChar32 value = IcuCodePoint(code_point);
  if (value == 0) return 0;
  if (!U_IS_UNICODE_CHAR(value) || u_iscntrl(value)) return -1;
  const int8_t category = u_charType(value);
  if (category == U_NON_SPACING_MARK || category == U_ENCLOSING_MARK ||
      category == U_FORMAT_CHAR) {
    return 0;
  }
  const int32_t width = u_getIntPropertyValue(value, UCHAR_EAST_ASIAN_WIDTH);
  return width == U_EA_WIDE || width == U_EA_FULLWIDTH ? 2 : 1;
}

extern "C" uint32_t aim_bionic_towupper_l(
    uint32_t code_point,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  if (code_point < 0x80) {
    return code_point >= 'a' && code_point <= 'z' ? code_point ^ 0x20
                                                  : code_point;
  }
  EnsureAndroidIcu76();
  return static_cast<uint32_t>(u_toupper(IcuCodePoint(code_point)));
}

extern "C" uint32_t aim_bionic_towupper(uint32_t code_point) {
  return aim_bionic_towupper_l(code_point, GlobalLocale());
}

extern "C" int aim_bionic_wcscoll_l(
    const uint32_t* left,
    const uint32_t* right,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  return WideCompare(left, right);
}

extern "C" int aim_bionic_wcscoll(const uint32_t* left,
                                           const uint32_t* right) {
  return aim_bionic_wcscoll_l(left, right, GlobalLocale());
}

extern "C" size_t aim_bionic_wcsxfrm_l(
    uint32_t* destination,
    const uint32_t* source,
    size_t length,
    AimAndroidLocale /*locale*/) {
  HostStateGuard guard;
  const size_t source_length = WideLength(source);
  if (length != 0) {
    const size_t copy = std::min(source_length, length - 1);
    std::memcpy(destination, source, copy * sizeof(uint32_t));
    destination[copy] = 0;
  }
  return source_length;
}

extern "C" size_t aim_bionic_wcsxfrm(uint32_t* destination,
                                             const uint32_t* source,
                                             size_t length) {
  return aim_bionic_wcsxfrm_l(destination, source, length,
                                     GlobalLocale());
}

namespace {
bool MatchPattern(const char* pattern, const char* text, int flags) {
  while (*pattern != '\0') {
    if (*pattern == '*') {
      while (*pattern == '*') ++pattern;
      if (*pattern == '\0')
        return (flags & 1) == 0 || std::strchr(text, '/') == nullptr;
      do {
        if (MatchPattern(pattern, text, flags)) return true;
      } while (*text != '\0' && ((flags & 1) == 0 || *text++ != '/'));
      return false;
    }
    if (*text == '\0') return false;
    if (*pattern == '?') {
      if ((flags & 1) != 0 && *text == '/') return false;
    } else if (*pattern == '\\' && (flags & 2) == 0 && pattern[1] != '\0') {
      ++pattern;
      if (*pattern != *text) return false;
    } else if (*pattern != *text) {
      return false;
    }
    ++pattern;
    ++text;
  }
  return *text == '\0';
}
}  // namespace

extern "C" int aim_bionic_fnmatch(const char* pattern,
                                           const char* text, int flags) {
  if (pattern == nullptr || text == nullptr) return 1;
  if ((flags & 4) != 0 && text[0] == '.' && pattern[0] != '.') return 1;
  return MatchPattern(pattern, text, flags) ? 0 : 1;
}

extern "C" void* aim_bionic_locale_resolve(const char* soname,
                                                    const char* symbol,
                                                    const char* version) {
  if (soname == nullptr || symbol == nullptr || version == nullptr ||
      std::string_view(soname) != "libc.so" ||
      std::string_view(version) != "LIBC") {
    return nullptr;
  }
#define RESOLVE(name)                                                        \
  if (std::string_view(symbol) == #name)                                     \
    return reinterpret_cast<void*>(&aim_bionic_##name)
  RESOLVE(_ctype_);
  RESOLVE(__ctype_get_mb_cur_max);
  RESOLVE(btowc);
  RESOLVE(freelocale);
  RESOLVE(fnmatch);
  RESOLVE(isalnum);
  RESOLVE(isalpha);
  RESOLVE(isdigit_l);
  RESOLVE(islower_l);
  RESOLVE(islower);
  RESOLVE(isupper_l);
  RESOLVE(isupper);
  RESOLVE(iswalpha_l);
  RESOLVE(iswalpha);
  RESOLVE(iswblank_l);
  RESOLVE(iswblank);
  RESOLVE(iswcntrl_l);
  RESOLVE(iswcntrl);
  RESOLVE(iswdigit_l);
  RESOLVE(iswdigit);
  RESOLVE(iswlower_l);
  RESOLVE(iswlower);
  RESOLVE(iswprint_l);
  RESOLVE(iswprint);
  RESOLVE(iswpunct_l);
  RESOLVE(iswpunct);
  RESOLVE(iswspace_l);
  RESOLVE(iswspace);
  RESOLVE(iswupper_l);
  RESOLVE(iswupper);
  RESOLVE(iswxdigit_l);
  RESOLVE(iswxdigit);
  RESOLVE(isxdigit_l);
  RESOLVE(isxdigit);
  RESOLVE(localeconv);
  RESOLVE(mbrlen);
  RESOLVE(mbrtowc);
  RESOLVE(mbsnrtowcs);
  RESOLVE(mbsrtowcs);
  RESOLVE(mbtowc);
  RESOLVE(newlocale);
  RESOLVE(setlocale);
  RESOLVE(strcoll_l);
  RESOLVE(strcoll);
  RESOLVE(strxfrm_l);
  RESOLVE(strxfrm);
  RESOLVE(tolower);
  RESOLVE(tolower_l);
  RESOLVE(towlower_l);
  RESOLVE(towlower);
  RESOLVE(towupper_l);
  RESOLVE(towupper);
  RESOLVE(toupper);
  RESOLVE(toupper_l);
  RESOLVE(uselocale);
  RESOLVE(wcrtomb);
  RESOLVE(wcscoll_l);
  RESOLVE(wcscoll);
  RESOLVE(wcsnrtombs);
  RESOLVE(wcsxfrm_l);
  RESOLVE(wcsxfrm);
  RESOLVE(wctob);
  RESOLVE(wcwidth);
#undef RESOLVE
  return nullptr;
}

extern "C" int aim_bionic_locale_capability(
    const char* capability) {
  if (capability == nullptr) return 0;
  const std::string_view name(capability);
  return name == "C" || name == "POSIX" || name == "C.UTF-8" ||
         name == "en_US.UTF-8" || name == "utf8-mbstate8" ||
         name == "C-collation" || name == "Unicode-wide-ctype" ||
         name == "Android-ICU-76.1";
}

extern "C" size_t aim_bionic_locale_live_handle_count(void) {
  ProviderState& state = State();
  std::lock_guard<std::mutex> lock(state.mutex);
  return state.locales.size();
}

extern "C" void aim_bionic_locale_test_prepare_host_state(void) {
  errno = 33101;
  fesetround(FE_DOWNWARD);
}

extern "C" int aim_bionic_locale_test_host_state_is_preserved(void) {
  return errno == 33101 && fegetround() == FE_DOWNWARD;
}

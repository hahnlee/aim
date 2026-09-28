#include <link.h>
#include <stddef.h>
#include <stdint.h>

__asm__(".symver dl_iterate_phdr,dl_iterate_phdr@LIBC");

static int StringEquals(const char* left, const char* right) {
  if (left == 0 || right == 0) return 0;
  while (*left != 0 && *left == *right) {
    ++left;
    ++right;
  }
  return *left == *right;
}

struct IterateState {
  int records;
  int load_segments;
  int saw_target;
  int saw_helper;
  int nested_records;
  uint64_t adds;
  uint64_t subs;
  int failed;
};

static int NestedCallback(struct dl_phdr_info* info, size_t size, void* opaque) {
  int* count = (int*)opaque;
  if (size != sizeof(struct dl_phdr_info) || info == 0 ||
      info->dlpi_addr == 0 || info->dlpi_name == 0 ||
      info->dlpi_phdr == 0 || info->dlpi_phnum == 0) {
    return 71;
  }
  ++*count;
  return 0;
}

static int FullCallback(struct dl_phdr_info* info, size_t size, void* opaque) {
  struct IterateState* state = (struct IterateState*)opaque;
  if (size != sizeof(struct dl_phdr_info) || info == 0 ||
      info->dlpi_addr == 0 || info->dlpi_name == 0 ||
      info->dlpi_phdr == 0 || info->dlpi_phnum == 0) {
    state->failed = 1;
    return 72;
  }
  if (state->records == 0) {
    state->adds = info->dlpi_adds;
    state->subs = info->dlpi_subs;
    if (dl_iterate_phdr(&NestedCallback, &state->nested_records) != 0) {
      state->failed = 2;
      return 73;
    }
  } else if (state->adds != info->dlpi_adds ||
             state->subs != info->dlpi_subs) {
    state->failed = 3;
    return 74;
  }
  ++state->records;
  state->saw_target |= StringEquals(info->dlpi_name, "libdl-phdr-target.so");
  state->saw_helper |= StringEquals(info->dlpi_name, "libdl-phdr-helper.so");
  for (ElfW(Half) index = 0; index < info->dlpi_phnum; ++index) {
    if (info->dlpi_phdr[index].p_type == PT_LOAD &&
        info->dlpi_phdr[index].p_memsz != 0) {
      ++state->load_segments;
    }
  }
  return 0;
}

static int EarlyCallback(struct dl_phdr_info* info, size_t size, void* opaque) {
  int* calls = (int*)opaque;
  if (info == 0 || size != sizeof(struct dl_phdr_info)) return 75;
  ++*calls;
  return 37;
}

__attribute__((visibility("default"))) int phdr_fixture_run(void) {
  struct IterateState state = {0};
  if (dl_iterate_phdr(&FullCallback, &state) != 0 || state.failed != 0) return -1;
  if (state.records != 2 || state.nested_records != 2 ||
      !state.saw_target || !state.saw_helper || state.load_segments < 4 ||
      state.adds != 2 || state.subs != 0) {
    return -2;
  }
  return 0;
}

__attribute__((visibility("default"))) int phdr_fixture_early_stop(void) {
  int calls = 0;
  const int result = dl_iterate_phdr(&EarlyCallback, &calls);
  return result == 37 && calls == 1 ? 0 : -3;
}

__attribute__((visibility("default"))) int phdr_fixture_after_unload(void) {
  struct IterateState state = {0};
  if (dl_iterate_phdr(&FullCallback, &state) != 0 || state.failed != 0) return -4;
  if (state.records != 1 || state.nested_records != 1 ||
      !state.saw_target || state.saw_helper || state.load_segments < 2 ||
      state.adds != 2 || state.subs != 1) {
    return -5;
  }
  return 0;
}

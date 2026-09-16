#include "darwin_art_bionic_libc_leaf.h"

#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

void* darwin_art_bionic_malloc(size_t size) { return malloc(size); }
void darwin_art_bionic_free(void* pointer) { free(pointer); }
_Noreturn void darwin_art_bionic___stack_chk_fail(void) { abort(); }

static int Sign(int value) { return (value > 0) - (value < 0); }

static int CompareInt(const void* left, const void* right) {
  const int a = *(const int*)left;
  const int b = *(const int*)right;
  return (a > b) - (a < b);
}

static uint32_t Next(uint32_t* state) {
  *state = *state * 1664525u + 1013904223u;
  return *state;
}

static void TestBionicContractEdges(void) {
  const unsigned char high[] = {0x80, 0};
  const unsigned char low[] = {0x7f, 0};
  assert(darwin_art_bionic_memcmp(high, low, 1) > 0);
  assert(darwin_art_bionic_strcmp((const char*)high, (const char*)low) > 0);
  assert(darwin_art_bionic_strcasecmp("ConScRyPt", "conscrypt") == 0);
  assert(darwin_art_bionic_strcasecmp("abc", "ABd") < 0);
  assert(darwin_art_bionic_strcasecmp((const char*)high, (const char*)low) > 0);
  assert(darwin_art_bionic_strncmp("same", "different", 0) == 0);
  assert(darwin_art_bionic_memchr("abc", 'z', 3) == NULL);
  assert(darwin_art_bionic_isprint(' ') != 0);
  assert(darwin_art_bionic_isprint('~') != 0);
  assert(darwin_art_bionic_isprint('\n') == 0);
  assert(darwin_art_bionic_isprint(0x80) == 0);
  assert(darwin_art_bionic_isprint(-1) == 0);

  char concatenated[8] = "abc";
  assert(darwin_art_bionic_strlcat(concatenated, "defgh", sizeof(concatenated)) ==
         8);
  assert(strcmp(concatenated, "abcdefg") == 0);
  char truncated[4] = {'a', 'b', 'c', 'd'};
  assert(darwin_art_bionic_strlcat(truncated, "xy", sizeof(truncated)) == 6);
  assert(memcmp(truncated, "abcd", sizeof(truncated)) == 0);
  assert(darwin_art_bionic_strlcat(NULL, "zero", 0) == 4);

  char checked[8] = {0};
  assert(darwin_art_bionic___memcpy_chk(checked, "abc", 4,
                                        sizeof(checked)) == checked);
  assert(darwin_art_bionic___memmove_chk(checked + 1, checked, 4,
                                         sizeof(checked) - 1) == checked + 1);
  assert(darwin_art_bionic___memset_chk(checked, 'x', 2,
                                        sizeof(checked)) == checked);
  unsigned char explicit_bytes[8];
  memset(explicit_bytes, 0xa5, sizeof(explicit_bytes));
  assert(darwin_art_bionic_memset_explicit(explicit_bytes, 0,
                                           sizeof(explicit_bytes)) ==
         explicit_bytes);
  for (size_t index = 0; index < sizeof(explicit_bytes); ++index)
    assert(explicit_bytes[index] == 0);
  explicit_bytes[0] = 0x3c;
  assert(darwin_art_bionic_memset_explicit(explicit_bytes, 0xff, 0) ==
         explicit_bytes);
  assert(explicit_bytes[0] == 0x3c);
  assert(darwin_art_bionic_memset_explicit(NULL, 0, 0) == NULL);
  int unsorted[] = {7, -2, 7, 0, 42, -100, 5};
  const int sorted[] = {-100, -2, 0, 5, 7, 7, 42};
  darwin_art_bionic_qsort(unsorted, sizeof(unsorted) / sizeof(unsorted[0]),
                          sizeof(unsorted[0]), CompareInt);
  assert(memcmp(unsorted, sorted, sizeof(sorted)) == 0);

  char overlap[] = "0123456789";
  assert(darwin_art_bionic_memmove(overlap + 2, overlap, 8) == overlap + 2);
  assert(memcmp(overlap, "0101234567", 10) == 0);
  assert(darwin_art_bionic_memmove(overlap, overlap + 2, 8) == overlap);
  assert(memcmp(overlap, "0123456767", 10) == 0);

  const wchar_t wide[] = {0x41, 0x20ac, 0x10ffff, 0};
  assert(darwin_art_bionic_wcslen(wide) == 3);
  assert(darwin_art_bionic_wmemchr(wide, 0x20ac, 3) == wide + 1);

  // Android arm64 wchar_t is unsigned while Darwin wchar_t is signed. The
  // facade must preserve Bionic ordering instead of forwarding this case.
  const wchar_t bionic_high[] = {(wchar_t)0xffffffffu};
  const wchar_t bionic_low[] = {0};
  assert(darwin_art_bionic_wmemcmp(bionic_high, bionic_low, 1) > 0);
  assert(wmemcmp(bionic_high, bionic_low, 1) < 0);
}

static void TestMemoryAndStrings(void) {
  uint32_t state = 0x4252f00du;
  for (size_t round = 0; round < 4096; ++round) {
    unsigned char left[257];
    unsigned char right[257];
    unsigned char facade[257];
    unsigned char darwin[257];
    for (size_t index = 0; index < sizeof(left); ++index) {
      left[index] = (unsigned char)Next(&state);
      right[index] = (unsigned char)Next(&state);
      facade[index] = (unsigned char)Next(&state);
      darwin[index] = facade[index];
    }
    const size_t length = Next(&state) % sizeof(left);
    assert(Sign(darwin_art_bionic_memcmp(left, right, length)) ==
           Sign(memcmp(left, right, length)));
    const int wanted = (int)Next(&state);
    assert(darwin_art_bionic_memchr(left, wanted, length) == memchr(left, wanted, length));
    assert(darwin_art_bionic_memcpy(facade, left, length) == facade);
    assert(memcpy(darwin, left, length) == darwin);
    assert(memcmp(facade, darwin, sizeof(facade)) == 0);
    assert(darwin_art_bionic_memset(facade, wanted, length) == facade);
    assert(memset(darwin, wanted, length) == darwin);
    assert(memcmp(facade, darwin, sizeof(facade)) == 0);

    const size_t source = Next(&state) % 128;
    const size_t destination = Next(&state) % 128;
    const size_t move_length = Next(&state) % 129;
    assert(darwin_art_bionic_memmove(facade + destination, facade + source, move_length) ==
           facade + destination);
    assert(memmove(darwin + destination, darwin + source, move_length) == darwin + destination);
    assert(memcmp(facade, darwin, sizeof(facade)) == 0);

    left[256] = 0;
    right[256] = 0;
    assert(darwin_art_bionic_strlen((const char*)left) == strlen((const char*)left));
    assert(Sign(darwin_art_bionic_strcmp((const char*)left, (const char*)right)) ==
           Sign(strcmp((const char*)left, (const char*)right)));
    assert(Sign(darwin_art_bionic_strncmp((const char*)left, (const char*)right, length)) ==
           Sign(strncmp((const char*)left, (const char*)right, length)));
  }
}

static void TestWideMemory(void) {
  const wchar_t left[] = {0, 1, 0x7f, 0x80, 0x20ac, 0x10ffff, 0};
  const wchar_t right[] = {0, 1, 0x7f, 0x80, 0x20ad, 0x10ffff, 0};
  assert(darwin_art_bionic_wcslen(left) == wcslen(left));
  assert(Sign(darwin_art_bionic_wmemcmp(left, right, 7)) == Sign(wmemcmp(left, right, 7)));
  for (size_t index = 0; index < 7; ++index) {
    assert(darwin_art_bionic_wmemchr(left, left[index], 7) == wmemchr(left, left[index], 7));
  }
}

static void TestResolver(void) {
  size_t count = 0;
  const DarwinArtBionicLeafBinding* table = darwin_art_bionic_libc_leaf_table(&count);
  assert(table != NULL && count == 67);
  for (size_t index = 0; index < count; ++index) {
    assert(table[index].address != NULL);
    assert(darwin_art_bionic_libc_leaf_resolve(table[index].import_name) == table[index].address);
    if (index != 0) assert(strcmp(table[index - 1].import_name, table[index].import_name) < 0);
  }
  assert(darwin_art_bionic_libc_leaf_resolve("malloc") == NULL);
  assert(darwin_art_bionic_libc_leaf_resolve("pthread_mutex_lock") == NULL);
  assert(darwin_art_bionic_libc_leaf_resolve("strtold") == NULL);
  assert(darwin_art_bionic_libc_leaf_resolve(NULL) == NULL);
}

int main(void) {
  TestBionicContractEdges();
  TestMemoryAndStrings();
  TestWideMemory();
  TestResolver();
  puts("bionic-libc-leaf differential: PASS cases=4096 bindings=67");
  return 0;
}

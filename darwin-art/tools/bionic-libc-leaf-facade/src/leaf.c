#include "darwin_art_bionic_libc_leaf.h"

#include <limits.h>

_Static_assert(CHAR_BIT == 8, "the facade requires 8-bit bytes");
_Static_assert(sizeof(wchar_t) == 4, "Bionic arm64 wchar_t is 32-bit");

void* darwin_art_bionic_memchr(const void* source, int value, size_t length) {
  const unsigned char* bytes = (const unsigned char*)source;
  const unsigned char wanted = (unsigned char)value;
  for (size_t index = 0; index < length; ++index) {
    if (bytes[index] == wanted) return (void*)(bytes + index);
  }
  return NULL;
}

void* darwin_art_bionic___memchr_chk(const void* source, int value,
                                     size_t length, size_t source_size) {
  if (length > source_size) return NULL;
  return darwin_art_bionic_memchr(source, value, length);
}

int darwin_art_bionic_memcmp(const void* left, const void* right, size_t length) {
  const unsigned char* a = (const unsigned char*)left;
  const unsigned char* b = (const unsigned char*)right;
  for (size_t index = 0; index < length; ++index) {
    if (a[index] != b[index]) return a[index] < b[index] ? -1 : 1;
  }
  return 0;
}

void* darwin_art_bionic_memcpy(void* destination, const void* source, size_t length) {
  unsigned char* output = (unsigned char*)destination;
  const unsigned char* input = (const unsigned char*)source;
  for (size_t index = 0; index < length; ++index) output[index] = input[index];
  return destination;
}

void* darwin_art_bionic___memcpy_chk(void* destination, const void* source,
                                     size_t length, size_t destination_size) {
  if (length > destination_size) __builtin_trap();
  return darwin_art_bionic_memcpy(destination, source, length);
}

void* darwin_art_bionic_memmove(void* destination, const void* source, size_t length) {
  unsigned char* output = (unsigned char*)destination;
  const unsigned char* input = (const unsigned char*)source;
  if ((uintptr_t)output <= (uintptr_t)input ||
      (uintptr_t)output - (uintptr_t)input >= length) {
    for (size_t index = 0; index < length; ++index) output[index] = input[index];
  } else {
    while (length != 0) {
      --length;
      output[length] = input[length];
    }
  }
  return destination;
}

void* darwin_art_bionic___memmove_chk(void* destination, const void* source,
                                      size_t length, size_t destination_size) {
  if (length > destination_size) __builtin_trap();
  return darwin_art_bionic_memmove(destination, source, length);
}

void* darwin_art_bionic_memset(void* destination, int value, size_t length) {
  unsigned char* output = (unsigned char*)destination;
  for (size_t index = 0; index < length; ++index) output[index] = (unsigned char)value;
  return destination;
}

void* darwin_art_bionic_memset_explicit(void* destination, int value,
                                        size_t length) {
  volatile unsigned char* output = (volatile unsigned char*)destination;
  for (size_t index = 0; index < length; ++index)
    output[index] = (unsigned char)value;
  return destination;
}

void* darwin_art_bionic___memset_chk(void* destination, int value,
                                     size_t length, size_t destination_size) {
  if (length > destination_size) __builtin_trap();
  return darwin_art_bionic_memset(destination, value, length);
}

int darwin_art_bionic_strcmp(const char* left, const char* right) {
  const unsigned char* a = (const unsigned char*)left;
  const unsigned char* b = (const unsigned char*)right;
  while (*a == *b && *a != 0) {
    ++a;
    ++b;
  }
  return *a == *b ? 0 : (*a < *b ? -1 : 1);
}

static unsigned char FoldAscii(unsigned char value) {
  return value >= 'A' && value <= 'Z' ? (unsigned char)(value + ('a' - 'A'))
                                      : value;
}

extern void darwin_art_bionic___stack_chk_fail(void)
    __attribute__((noreturn));

int darwin_art_bionic_strcasecmp(const char* left, const char* right) {
  const unsigned char* a = (const unsigned char*)left;
  const unsigned char* b = (const unsigned char*)right;
  for (;;) {
    const unsigned char folded_a = FoldAscii(*a);
    const unsigned char folded_b = FoldAscii(*b);
    if (folded_a != folded_b) return (int)folded_a - (int)folded_b;
    if (*a == 0) return 0;
    ++a;
    ++b;
  }
}

int darwin_art_bionic_strncasecmp(const char* left, const char* right,
                                  size_t length) {
  const unsigned char* a = (const unsigned char*)left;
  const unsigned char* b = (const unsigned char*)right;
  while (length-- != 0) {
    const unsigned char folded_a = FoldAscii(*a);
    const unsigned char folded_b = FoldAscii(*b);
    if (folded_a != folded_b) return (int)folded_a - (int)folded_b;
    if (*a == 0) return 0;
    ++a;
    ++b;
  }
  return 0;
}

int darwin_art_bionic_isspace(int value) {
  const unsigned char character = (unsigned char)value;
  return character == ' ' || character == '\t' || character == '\n' ||
         character == '\v' || character == '\f' || character == '\r';
}

int darwin_art_bionic_isprint(int value) {
  const unsigned char character = (unsigned char)value;
  return value != -1 && character >= 0x20 && character <= 0x7e;
}

size_t darwin_art_bionic_strnlen(const char* string, size_t maximum) {
  size_t length = 0;
  while (length < maximum && string[length] != '\0') ++length;
  return length;
}

size_t darwin_art_bionic_strlcat(char* destination, const char* source,
                                 size_t size) {
  const size_t destination_length =
      darwin_art_bionic_strnlen(destination, size);
  size_t source_length = 0;
  while (source[source_length] != '\0') ++source_length;
  if (destination_length == size) return size + source_length;

  const size_t available = size - destination_length;
  const size_t copied = source_length < available - 1 ? source_length
                                                       : available - 1;
  for (size_t index = 0; index < copied; ++index)
    destination[destination_length + index] = source[index];
  destination[destination_length + copied] = '\0';
  return destination_length + source_length;
}

size_t darwin_art_bionic_strlcpy(char* destination, const char* source,
                                 size_t size) {
  size_t source_length = 0;
  while (source[source_length] != '\0') ++source_length;
  if (size != 0) {
    const size_t copied = source_length < size - 1 ? source_length : size - 1;
    for (size_t index = 0; index < copied; ++index)
      destination[index] = source[index];
    destination[copied] = '\0';
  }
  return source_length;
}

char* darwin_art_bionic_stpcpy(char* destination, const char* source) {
  while ((*destination = *source) != '\0') {
    ++destination;
    ++source;
  }
  return destination;
}

char* darwin_art_bionic_strcasestr(const char* haystack, const char* needle) {
  if (*needle == '\0') return (char*)haystack;
  for (const char* candidate = haystack; *candidate != '\0'; ++candidate) {
    const unsigned char* left = (const unsigned char*)candidate;
    const unsigned char* right = (const unsigned char*)needle;
    while (*left != 0 && *right != 0 &&
           FoldAscii(*left) == FoldAscii(*right)) {
      ++left;
      ++right;
    }
    if (*right == 0) return (char*)candidate;
  }
  return NULL;
}

void* darwin_art_bionic_memrchr(const void* memory, int value, size_t length) {
  const unsigned char* bytes = (const unsigned char*)memory;
  const unsigned char wanted = (unsigned char)value;
  while (length != 0) {
    --length;
    if (bytes[length] == wanted) return (void*)(bytes + length);
  }
  return NULL;
}

void* darwin_art_bionic_memmem(const void* haystack, size_t haystack_length,
                               const void* needle, size_t needle_length) {
  if (needle_length == 0) return (void*)haystack;
  if (needle_length > haystack_length) return NULL;
  const unsigned char* bytes = (const unsigned char*)haystack;
  for (size_t offset = 0; offset <= haystack_length - needle_length; ++offset) {
    if (darwin_art_bionic_memcmp(bytes + offset, needle, needle_length) == 0)
      return (void*)(bytes + offset);
  }
  return NULL;
}

void* darwin_art_bionic_lfind(const void* key, const void* base,
                              size_t* count, size_t width,
                              int (*compare)(const void*, const void*)) {
  if (key == NULL || base == NULL || count == NULL || compare == NULL)
    return NULL;
  const unsigned char* bytes = (const unsigned char*)base;
  for (size_t index = 0; index < *count; ++index) {
    const void* candidate = bytes + index * width;
    if (compare(key, candidate) == 0) return (void*)candidate;
  }
  return NULL;
}

typedef struct DarwinArtBionicSearchNode {
  const void* key;
  struct DarwinArtBionicSearchNode* left;
  struct DarwinArtBionicSearchNode* right;
} DarwinArtBionicSearchNode;

extern void* darwin_art_bionic_malloc(size_t size);
extern void darwin_art_bionic_free(void* pointer);

void* darwin_art_bionic_tfind(const void* key, void* const* root,
                              int (*compare)(const void*, const void*)) {
  if (root == NULL || compare == NULL) return NULL;
  DarwinArtBionicSearchNode* node = *(DarwinArtBionicSearchNode* const*)root;
  while (node != NULL) {
    const int order = compare(key, node->key);
    if (order == 0) return node;
    node = order < 0 ? node->left : node->right;
  }
  return NULL;
}

void* darwin_art_bionic_tsearch(const void* key, void** root,
                                int (*compare)(const void*, const void*)) {
  if (root == NULL || compare == NULL) return NULL;
  DarwinArtBionicSearchNode** link = (DarwinArtBionicSearchNode**)root;
  while (*link != NULL) {
    const int order = compare(key, (*link)->key);
    if (order == 0) return *link;
    link = order < 0 ? &(*link)->left : &(*link)->right;
  }
  DarwinArtBionicSearchNode* node =
      (DarwinArtBionicSearchNode*)darwin_art_bionic_malloc(sizeof(*node));
  if (node == NULL) return NULL;
  node->key = key;
  node->left = NULL;
  node->right = NULL;
  *link = node;
  return node;
}

void* darwin_art_bionic_tdelete(const void* key, void** root,
                                int (*compare)(const void*, const void*)) {
  if (root == NULL || compare == NULL) return NULL;
  DarwinArtBionicSearchNode** link = (DarwinArtBionicSearchNode**)root;
  DarwinArtBionicSearchNode* parent = NULL;
  while (*link != NULL) {
    const int order = compare(key, (*link)->key);
    if (order == 0) break;
    parent = *link;
    link = order < 0 ? &(*link)->left : &(*link)->right;
  }
  DarwinArtBionicSearchNode* removed = *link;
  if (removed == NULL) return NULL;
  if (removed->left == NULL) {
    *link = removed->right;
  } else if (removed->right == NULL) {
    *link = removed->left;
  } else {
    DarwinArtBionicSearchNode** successor_link = &removed->right;
    DarwinArtBionicSearchNode* successor_parent = removed;
    while ((*successor_link)->left != NULL) {
      successor_parent = *successor_link;
      successor_link = &(*successor_link)->left;
    }
    DarwinArtBionicSearchNode* successor = *successor_link;
    *successor_link = successor->right;
    successor->left = removed->left;
    if (successor != removed->right) successor->right = removed->right;
    *link = successor;
    if (successor_parent == removed) successor_parent = successor;
    parent = successor_parent;
  }
  darwin_art_bionic_free(removed);
  return parent != NULL ? parent : *root;
}

static void WalkSearchTree(const DarwinArtBionicSearchNode* node,
                           void (*action)(const void*, int, int), int depth) {
  if (node->left == NULL && node->right == NULL) {
    action(node, 3, depth);
    return;
  }
  action(node, 0, depth);
  if (node->left != NULL) WalkSearchTree(node->left, action, depth + 1);
  action(node, 1, depth);
  if (node->right != NULL) WalkSearchTree(node->right, action, depth + 1);
  action(node, 2, depth);
}

void darwin_art_bionic_twalk(const void* root,
                             void (*action)(const void*, int, int)) {
  if (root != NULL && action != NULL)
    WalkSearchTree((const DarwinArtBionicSearchNode*)root, action, 0);
}

void darwin_art_bionic_tdestroy(void* root, void (*free_key)(void*)) {
  DarwinArtBionicSearchNode* node = (DarwinArtBionicSearchNode*)root;
  if (node == NULL) return;
  darwin_art_bionic_tdestroy(node->left, free_key);
  darwin_art_bionic_tdestroy(node->right, free_key);
  if (free_key != NULL) free_key((void*)node->key);
  darwin_art_bionic_free(node);
}

char* darwin_art_bionic___strcat_chk(char* destination, const char* source,
                                     size_t destination_size) {
  const size_t left = darwin_art_bionic_strlen(destination);
  const size_t right = darwin_art_bionic_strlen(source);
  if (left >= destination_size || right >= destination_size - left)
    darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strcat(destination, source);
}

char* darwin_art_bionic___strcpy_chk(char* destination, const char* source,
                                     size_t destination_size) {
  if (darwin_art_bionic_strlen(source) >= destination_size)
    darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strcpy(destination, source);
}

char* darwin_art_bionic___strncat_chk(char* destination, const char* source,
                                      size_t length,
                                      size_t destination_size) {
  const size_t left = darwin_art_bionic_strlen(destination);
  const size_t copied = darwin_art_bionic_strnlen(source, length);
  if (left >= destination_size || copied >= destination_size - left)
    darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strncat(destination, source, length);
}

char* darwin_art_bionic___strncpy_chk(char* destination, const char* source,
                                      size_t length,
                                      size_t destination_size) {
  if (length > destination_size) darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strncpy(destination, source, length);
}

char* darwin_art_bionic___strncpy_chk2(char* destination, const char* source,
                                       size_t length, size_t destination_size,
                                       size_t source_size) {
  if (length > destination_size ||
      darwin_art_bionic_strnlen(source, length) == source_size)
    darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strncpy(destination, source, length);
}

size_t darwin_art_bionic___strlcpy_chk(char* destination, const char* source,
                                       size_t size,
                                       size_t destination_size) {
  if (size > destination_size) darwin_art_bionic___stack_chk_fail();
  return darwin_art_bionic_strlcpy(destination, source, size);
}

char* darwin_art_bionic___strchr_chk(const char* string, int value,
                                     size_t string_size) {
  if (darwin_art_bionic_strnlen(string, string_size) == string_size)
    darwin_art_bionic___stack_chk_fail();
  return (char*)darwin_art_bionic_strchr(string, value);
}

char* darwin_art_bionic___strrchr_chk(const char* string, int value,
                                      size_t string_size) {
  if (darwin_art_bionic_strnlen(string, string_size) == string_size)
    darwin_art_bionic___stack_chk_fail();
  return (char*)darwin_art_bionic_strrchr(string, value);
}

const char* darwin_art_bionic_strchr(const char* string, int value) {
  const unsigned char wanted = (unsigned char)value;
  for (const unsigned char* cursor = (const unsigned char*)string;; ++cursor) {
    if (*cursor == wanted) return (const char*)cursor;
    if (*cursor == 0) return NULL;
  }
}

size_t darwin_art_bionic_strlen(const char* string) {
  const char* end = string;
  while (*end != '\0') ++end;
  return (size_t)(end - string);
}

size_t darwin_art_bionic___strlen_chk(const char* string, size_t buffer_size) {
  const size_t length = darwin_art_bionic_strlen(string);
  if (length >= buffer_size) __builtin_trap();
  return length;
}

int darwin_art_bionic_strncmp(const char* left, const char* right, size_t length) {
  const unsigned char* a = (const unsigned char*)left;
  const unsigned char* b = (const unsigned char*)right;
  while (length != 0) {
    if (*a != *b) return *a < *b ? -1 : 1;
    if (*a == 0) return 0;
    ++a;
    ++b;
    --length;
  }
  return 0;
}

const char* darwin_art_bionic_strstr(const char* haystack, const char* needle) {
  if (*needle == '\0') return haystack;
  for (const char* candidate = haystack; *candidate != '\0'; ++candidate) {
    const char* a = candidate;
    const char* b = needle;
    while (*a != '\0' && *b != '\0' && *a == *b) {
      ++a;
      ++b;
    }
    if (*b == '\0') return candidate;
  }
  return NULL;
}

static int Contains(const char* set, unsigned char value) {
  for (; *set != '\0'; ++set) if ((unsigned char)*set == value) return 1;
  return 0;
}

char* darwin_art_bionic_strsep(char** string, const char* delimiters) {
  if (string == NULL || *string == NULL) return NULL;
  char* token = *string;
  char* cursor = token;
  while (*cursor != '\0') {
    if (Contains(delimiters, (unsigned char)*cursor)) {
      *cursor = '\0';
      *string = cursor + 1;
      return token;
    }
    ++cursor;
  }
  *string = NULL;
  return token;
}

char* darwin_art_bionic_strtok_r(char* string, const char* delimiters,
                                 char** state) {
  if (state == NULL) return NULL;
  char* cursor = string != NULL ? string : *state;
  if (cursor == NULL) return NULL;
  while (*cursor != '\0' && Contains(delimiters, (unsigned char)*cursor))
    ++cursor;
  if (*cursor == '\0') {
    *state = cursor;
    return NULL;
  }
  char* token = cursor;
  while (*cursor != '\0' && !Contains(delimiters, (unsigned char)*cursor))
    ++cursor;
  if (*cursor != '\0') *cursor++ = '\0';
  *state = cursor;
  return token;
}

char* darwin_art_bionic_strtok(char* string, const char* delimiters) {
  static _Thread_local char* state;
  return darwin_art_bionic_strtok_r(string, delimiters, &state);
}

void darwin_art_bionic___FD_SET_chk(int fd, unsigned long* set,
                                    size_t set_size) {
  if (fd < 0 || (size_t)fd >= set_size * 8)
    darwin_art_bionic___stack_chk_fail();
  set[(size_t)fd / (sizeof(unsigned long) * 8)] |=
      1ul << ((size_t)fd % (sizeof(unsigned long) * 8));
}

void darwin_art_bionic___FD_CLR_chk(int fd, unsigned long* set,
                                    size_t set_size) {
  if (fd < 0 || (size_t)fd >= set_size * 8)
    darwin_art_bionic___stack_chk_fail();
  set[(size_t)fd / (sizeof(unsigned long) * 8)] &=
      ~(1ul << ((size_t)fd % (sizeof(unsigned long) * 8)));
}

int darwin_art_bionic___FD_ISSET_chk(int fd, const unsigned long* set,
                                     size_t set_size) {
  if (fd < 0 || (size_t)fd >= set_size * 8)
    darwin_art_bionic___stack_chk_fail();
  return (set[(size_t)fd / (sizeof(unsigned long) * 8)] &
          (1ul << ((size_t)fd % (sizeof(unsigned long) * 8)))) != 0;
}

wchar_t* darwin_art_bionic_wcschr(const wchar_t* string, wchar_t value) {
  for (;; ++string) {
    if (*string == value) return (wchar_t*)string;
    if (*string == 0) return NULL;
  }
}

wchar_t* darwin_art_bionic_wcscpy(wchar_t* destination,
                                  const wchar_t* source) {
  wchar_t* result = destination;
  while ((*destination++ = *source++) != 0) {}
  return result;
}

size_t darwin_art_bionic_strspn(const char* string, const char* accept) {
  size_t length = 0;
  while (string[length] != '\0' && Contains(accept, (unsigned char)string[length])) ++length;
  return length;
}

size_t darwin_art_bionic_strcspn(const char* string, const char* reject) {
  size_t length = 0;
  while (string[length] != '\0' && !Contains(reject, (unsigned char)string[length])) ++length;
  return length;
}

const char* darwin_art_bionic_strpbrk(const char* string, const char* accept) {
  for (; *string != '\0'; ++string) if (Contains(accept, (unsigned char)*string)) return string;
  return NULL;
}

const char* darwin_art_bionic_strrchr(const char* string, int value) {
  const char* found = NULL;
  const unsigned char wanted = (unsigned char)value;
  for (const unsigned char* cursor = (const unsigned char*)string;; ++cursor) {
    if (*cursor == wanted) found = (const char*)cursor;
    if (*cursor == 0) return found;
  }
}

char* darwin_art_bionic_strcpy(char* destination, const char* source) {
  char* result = destination;
  while ((*destination++ = *source++) != '\0') {}
  return result;
}

int darwin_art_bionic_atoi(const char* string) {
  while (*string == ' ' || *string == '\t' || *string == '\n' || *string == '\r') ++string;
  int sign = 1;
  if (*string == '-' || *string == '+') { if (*string++ == '-') sign = -1; }
  int value = 0;
  while (*string >= '0' && *string <= '9') value = value * 10 + (*string++ - '0');
  return sign * value;
}

long darwin_art_bionic_atol(const char* string) {
  while (*string == ' ' || *string == '\t' || *string == '\n' ||
         *string == '\r' || *string == '\f' || *string == '\v') {
    ++string;
  }
  int negative = 0;
  if (*string == '-' || *string == '+') negative = *string++ == '-';
  unsigned long value = 0;
  while (*string >= '0' && *string <= '9') {
    value = value * 10 + (unsigned long)(*string++ - '0');
  }
  return negative ? -(long)value : (long)value;
}

char* darwin_art_bionic_strcat(char* destination, const char* source) {
  char* result = destination;
  while (*destination != '\0') ++destination;
  while ((*destination++ = *source++) != '\0') {}
  return result;
}

char* darwin_art_bionic_strncat(char* destination, const char* source,
                                size_t length) {
  char* result = destination;
  while (*destination != '\0') ++destination;
  while (length != 0 && *source != '\0') {
    *destination++ = *source++;
    --length;
  }
  *destination = '\0';
  return result;
}

char* darwin_art_bionic_strncpy(char* destination, const char* source,
                                size_t length) {
  char* result = destination;
  while (length != 0 && *source != '\0') {
    *destination++ = *source++;
    --length;
  }
  while (length-- != 0) *destination++ = '\0';
  return result;
}

void* darwin_art_bionic_bsearch(const void* key, const void* base, size_t count,
                                size_t size, int (*compare)(const void*, const void*)) {
  const unsigned char* bytes = (const unsigned char*)base;
  size_t low = 0;
  while (low < count) {
    const size_t middle = low + (count - low) / 2;
    const void* candidate = bytes + middle * size;
    const int order = compare(key, candidate);
    if (order == 0) return (void*)candidate;
    if (order < 0) count = middle;
    else low = middle + 1;
  }
  return NULL;
}

static void SwapElements(unsigned char* left, unsigned char* right,
                         size_t size) {
  if (left == right) return;
  for (size_t index = 0; index < size; ++index) {
    const unsigned char byte = left[index];
    left[index] = right[index];
    right[index] = byte;
  }
}

static void SiftDown(unsigned char* bytes, size_t root, size_t count,
                     size_t size, int (*compare)(const void*, const void*)) {
  while (root < count / 2) {
    size_t child = root * 2 + 1;
    if (child + 1 < count &&
        compare(bytes + child * size, bytes + (child + 1) * size) < 0) {
      ++child;
    }
    if (compare(bytes + root * size, bytes + child * size) >= 0) return;
    SwapElements(bytes + root * size, bytes + child * size, size);
    root = child;
  }
}

void darwin_art_bionic_qsort(void* base, size_t count, size_t size,
                             int (*compare)(const void*, const void*)) {
  if (count < 2 || size == 0) return;
  if (base == NULL || compare == NULL || count > SIZE_MAX / size) {
    __builtin_trap();
  }
  unsigned char* bytes = (unsigned char*)base;
  for (size_t start = count / 2; start != 0; --start) {
    SiftDown(bytes, start - 1, count, size, compare);
  }
  for (size_t end = count; end > 1; --end) {
    SwapElements(bytes, bytes + (end - 1) * size, size);
    SiftDown(bytes, 0, end - 1, size, compare);
  }
}

wchar_t* darwin_art_bionic_wmemchr(const wchar_t* source, wchar_t value, size_t length) {
  for (size_t index = 0; index < length; ++index) {
    if (source[index] == value) return (wchar_t*)(source + index);
  }
  return NULL;
}

int darwin_art_bionic_wmemcmp(const wchar_t* left, const wchar_t* right, size_t length) {
  for (size_t index = 0; index < length; ++index) {
    const uint32_t a = (uint32_t)left[index];
    const uint32_t b = (uint32_t)right[index];
    if (a != b) return a < b ? -1 : 1;
  }
  return 0;
}

size_t darwin_art_bionic_wcslen(const wchar_t* string) {
  const wchar_t* end = string;
  while (*end != 0) ++end;
  return (size_t)(end - string);
}

wchar_t* darwin_art_bionic_wmemcpy(wchar_t* destination,
                                    const wchar_t* source, size_t length) {
  for (size_t index = 0; index < length; ++index) destination[index] = source[index];
  return destination;
}

wchar_t* darwin_art_bionic_wmemmove(wchar_t* destination,
                                     const wchar_t* source, size_t length) {
  if ((uintptr_t)destination <= (uintptr_t)source ||
      (uintptr_t)destination - (uintptr_t)source >= length * sizeof(wchar_t)) {
    return darwin_art_bionic_wmemcpy(destination, source, length);
  }
  while (length != 0) {
    --length;
    destination[length] = source[length];
  }
  return destination;
}

wchar_t* darwin_art_bionic_wmemset(wchar_t* destination, wchar_t value,
                                    size_t length) {
  for (size_t index = 0; index < length; ++index) destination[index] = value;
  return destination;
}

static int NameCompare(const char* left, const char* right) {
  while (*left == *right && *left != '\0') {
    ++left;
    ++right;
  }
  return (unsigned char)*left < (unsigned char)*right
             ? -1
             : ((unsigned char)*left != (unsigned char)*right);
}

static const DarwinArtBionicLeafBinding kBindings[] = {
    {"__FD_CLR_chk", (DarwinArtBionicFunction)darwin_art_bionic___FD_CLR_chk},
    {"__FD_ISSET_chk", (DarwinArtBionicFunction)darwin_art_bionic___FD_ISSET_chk},
    {"__FD_SET_chk", (DarwinArtBionicFunction)darwin_art_bionic___FD_SET_chk},
    {"__memchr_chk", (DarwinArtBionicFunction)darwin_art_bionic___memchr_chk},
    {"__memcpy_chk", (DarwinArtBionicFunction)darwin_art_bionic___memcpy_chk},
    {"__memmove_chk", (DarwinArtBionicFunction)darwin_art_bionic___memmove_chk},
    {"__memset_chk", (DarwinArtBionicFunction)darwin_art_bionic___memset_chk},
    {"__strcat_chk", (DarwinArtBionicFunction)darwin_art_bionic___strcat_chk},
    {"__strchr_chk", (DarwinArtBionicFunction)darwin_art_bionic___strchr_chk},
    {"__strcpy_chk", (DarwinArtBionicFunction)darwin_art_bionic___strcpy_chk},
    {"__strlcpy_chk", (DarwinArtBionicFunction)darwin_art_bionic___strlcpy_chk},
    {"__strlen_chk", (DarwinArtBionicFunction)darwin_art_bionic___strlen_chk},
    {"__strncat_chk", (DarwinArtBionicFunction)darwin_art_bionic___strncat_chk},
    {"__strncpy_chk", (DarwinArtBionicFunction)darwin_art_bionic___strncpy_chk},
    {"__strncpy_chk2", (DarwinArtBionicFunction)darwin_art_bionic___strncpy_chk2},
    {"__strrchr_chk", (DarwinArtBionicFunction)darwin_art_bionic___strrchr_chk},
    {"atoi", (DarwinArtBionicFunction)darwin_art_bionic_atoi},
    {"atol", (DarwinArtBionicFunction)darwin_art_bionic_atol},
    {"bsearch", (DarwinArtBionicFunction)darwin_art_bionic_bsearch},
    {"isprint", (DarwinArtBionicFunction)darwin_art_bionic_isprint},
    {"isspace", (DarwinArtBionicFunction)darwin_art_bionic_isspace},
    {"lfind", (DarwinArtBionicFunction)darwin_art_bionic_lfind},
    {"memchr", (DarwinArtBionicFunction)darwin_art_bionic_memchr},
    {"memcmp", (DarwinArtBionicFunction)darwin_art_bionic_memcmp},
    {"memcpy", (DarwinArtBionicFunction)darwin_art_bionic_memcpy},
    {"memmem", (DarwinArtBionicFunction)darwin_art_bionic_memmem},
    {"memmove", (DarwinArtBionicFunction)darwin_art_bionic_memmove},
    {"memrchr", (DarwinArtBionicFunction)darwin_art_bionic_memrchr},
    {"memset", (DarwinArtBionicFunction)darwin_art_bionic_memset},
    {"memset_explicit",
     (DarwinArtBionicFunction)darwin_art_bionic_memset_explicit},
    {"qsort", (DarwinArtBionicFunction)darwin_art_bionic_qsort},
    {"stpcpy", (DarwinArtBionicFunction)darwin_art_bionic_stpcpy},
    {"strcasecmp", (DarwinArtBionicFunction)darwin_art_bionic_strcasecmp},
    {"strcasestr", (DarwinArtBionicFunction)darwin_art_bionic_strcasestr},
    {"strcat", (DarwinArtBionicFunction)darwin_art_bionic_strcat},
    {"strchr", (DarwinArtBionicFunction)darwin_art_bionic_strchr},
    {"strcmp", (DarwinArtBionicFunction)darwin_art_bionic_strcmp},
    {"strcpy", (DarwinArtBionicFunction)darwin_art_bionic_strcpy},
    {"strcspn", (DarwinArtBionicFunction)darwin_art_bionic_strcspn},
    {"strlcat", (DarwinArtBionicFunction)darwin_art_bionic_strlcat},
    {"strlcpy", (DarwinArtBionicFunction)darwin_art_bionic_strlcpy},
    {"strlen", (DarwinArtBionicFunction)darwin_art_bionic_strlen},
    {"strncasecmp", (DarwinArtBionicFunction)darwin_art_bionic_strncasecmp},
    {"strncat", (DarwinArtBionicFunction)darwin_art_bionic_strncat},
    {"strncmp", (DarwinArtBionicFunction)darwin_art_bionic_strncmp},
    {"strncpy", (DarwinArtBionicFunction)darwin_art_bionic_strncpy},
    {"strnlen", (DarwinArtBionicFunction)darwin_art_bionic_strnlen},
    {"strpbrk", (DarwinArtBionicFunction)darwin_art_bionic_strpbrk},
    {"strrchr", (DarwinArtBionicFunction)darwin_art_bionic_strrchr},
    {"strsep", (DarwinArtBionicFunction)darwin_art_bionic_strsep},
    {"strspn", (DarwinArtBionicFunction)darwin_art_bionic_strspn},
    {"strstr", (DarwinArtBionicFunction)darwin_art_bionic_strstr},
    {"strtok", (DarwinArtBionicFunction)darwin_art_bionic_strtok},
    {"strtok_r", (DarwinArtBionicFunction)darwin_art_bionic_strtok_r},
    {"tdelete", (DarwinArtBionicFunction)darwin_art_bionic_tdelete},
    {"tdestroy", (DarwinArtBionicFunction)darwin_art_bionic_tdestroy},
    {"tfind", (DarwinArtBionicFunction)darwin_art_bionic_tfind},
    {"tsearch", (DarwinArtBionicFunction)darwin_art_bionic_tsearch},
    {"twalk", (DarwinArtBionicFunction)darwin_art_bionic_twalk},
    {"wcschr", (DarwinArtBionicFunction)darwin_art_bionic_wcschr},
    {"wcscpy", (DarwinArtBionicFunction)darwin_art_bionic_wcscpy},
    {"wcslen", (DarwinArtBionicFunction)darwin_art_bionic_wcslen},
    {"wmemchr", (DarwinArtBionicFunction)darwin_art_bionic_wmemchr},
    {"wmemcmp", (DarwinArtBionicFunction)darwin_art_bionic_wmemcmp},
    {"wmemcpy", (DarwinArtBionicFunction)darwin_art_bionic_wmemcpy},
    {"wmemmove", (DarwinArtBionicFunction)darwin_art_bionic_wmemmove},
    {"wmemset", (DarwinArtBionicFunction)darwin_art_bionic_wmemset},
};

const DarwinArtBionicLeafBinding* darwin_art_bionic_libc_leaf_table(size_t* count) {
  if (count != NULL) *count = sizeof(kBindings) / sizeof(kBindings[0]);
  return kBindings;
}

DarwinArtBionicFunction darwin_art_bionic_libc_leaf_resolve(const char* import_name) {
  if (import_name == NULL) return NULL;
  size_t low = 0;
  size_t high = sizeof(kBindings) / sizeof(kBindings[0]);
  while (low < high) {
    const size_t middle = low + (high - low) / 2;
    const int order = NameCompare(import_name, kBindings[middle].import_name);
    if (order == 0) return kBindings[middle].address;
    if (order < 0)
      high = middle;
    else
      low = middle + 1;
  }
  return NULL;
}

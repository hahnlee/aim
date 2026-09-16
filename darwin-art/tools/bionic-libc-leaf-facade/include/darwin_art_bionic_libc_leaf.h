#ifndef DARWIN_ART_BIONIC_LIBC_LEAF_H_
#define DARWIN_ART_BIONIC_LIBC_LEAF_H_

#include <stddef.h>
#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*DarwinArtBionicFunction)(void);

typedef struct DarwinArtBionicLeafBinding {
  const char* import_name;
  DarwinArtBionicFunction address;
} DarwinArtBionicLeafBinding;

void* darwin_art_bionic_memchr(const void* source, int value, size_t length);
int darwin_art_bionic_memcmp(const void* left, const void* right, size_t length);
void* darwin_art_bionic_memcpy(void* destination, const void* source, size_t length);
void* darwin_art_bionic___memcpy_chk(void* destination, const void* source,
                                     size_t length, size_t destination_size);
void* darwin_art_bionic_memmove(void* destination, const void* source, size_t length);
void* darwin_art_bionic___memmove_chk(void* destination, const void* source,
                                      size_t length, size_t destination_size);
void* darwin_art_bionic_memset(void* destination, int value, size_t length);
void* darwin_art_bionic_memset_explicit(void* destination, int value,
                                        size_t length);
void* darwin_art_bionic___memset_chk(void* destination, int value,
                                     size_t length, size_t destination_size);
int darwin_art_bionic_strcmp(const char* left, const char* right);
int darwin_art_bionic_strcasecmp(const char* left, const char* right);
int darwin_art_bionic_strncasecmp(const char* left, const char* right,
                                  size_t length);
int darwin_art_bionic_isspace(int value);
int darwin_art_bionic_isprint(int value);
size_t darwin_art_bionic_strnlen(const char* string, size_t maximum);
size_t darwin_art_bionic_strlcat(char* destination, const char* source,
                                 size_t size);
size_t darwin_art_bionic_strlcpy(char* destination, const char* source,
                                 size_t size);
char* darwin_art_bionic_stpcpy(char* destination, const char* source);
char* darwin_art_bionic_strcasestr(const char* haystack, const char* needle);
char* darwin_art_bionic_strsep(char** string, const char* delimiters);
char* darwin_art_bionic_strtok_r(char* string, const char* delimiters,
                                 char** state);
char* darwin_art_bionic_strtok(char* string, const char* delimiters);
void* darwin_art_bionic_memrchr(const void* memory, int value, size_t length);
void* darwin_art_bionic_memmem(const void* haystack, size_t haystack_length,
                               const void* needle, size_t needle_length);
void* darwin_art_bionic_lfind(const void* key, const void* base,
                              size_t* count, size_t width,
                              int (*compare)(const void*, const void*));
void* darwin_art_bionic_tsearch(const void* key, void** root,
                                int (*compare)(const void*, const void*));
void* darwin_art_bionic_tfind(const void* key, void* const* root,
                              int (*compare)(const void*, const void*));
void* darwin_art_bionic_tdelete(const void* key, void** root,
                                int (*compare)(const void*, const void*));
void darwin_art_bionic_twalk(const void* root,
                             void (*action)(const void*, int, int));
void darwin_art_bionic_tdestroy(void* root, void (*free_key)(void*));
const char* darwin_art_bionic_strchr(const char* string, int value);
size_t darwin_art_bionic_strlen(const char* string);
size_t darwin_art_bionic___strlen_chk(const char* string, size_t buffer_size);
int darwin_art_bionic_strncmp(const char* left, const char* right, size_t length);
const char* darwin_art_bionic_strstr(const char* haystack, const char* needle);
size_t darwin_art_bionic_strspn(const char* string, const char* accept);
size_t darwin_art_bionic_strcspn(const char* string, const char* reject);
const char* darwin_art_bionic_strpbrk(const char* string, const char* accept);
const char* darwin_art_bionic_strrchr(const char* string, int value);
char* darwin_art_bionic_strcpy(char* destination, const char* source);
int darwin_art_bionic_atoi(const char* string);
long darwin_art_bionic_atol(const char* string);
char* darwin_art_bionic_strcat(char* destination, const char* source);
char* darwin_art_bionic_strncat(char* destination, const char* source,
                                size_t length);
char* darwin_art_bionic_strncpy(char* destination, const char* source,
                                size_t length);
void* darwin_art_bionic_bsearch(const void* key, const void* base, size_t count,
                                size_t size, int (*compare)(const void*, const void*));
void darwin_art_bionic_qsort(void* base, size_t count, size_t size,
                             int (*compare)(const void*, const void*));
wchar_t* darwin_art_bionic_wmemchr(const wchar_t* source, wchar_t value, size_t length);
int darwin_art_bionic_wmemcmp(const wchar_t* left, const wchar_t* right, size_t length);
size_t darwin_art_bionic_wcslen(const wchar_t* string);
wchar_t* darwin_art_bionic_wmemcpy(wchar_t* destination,
                                   const wchar_t* source, size_t length);
wchar_t* darwin_art_bionic_wmemmove(wchar_t* destination,
                                    const wchar_t* source, size_t length);
wchar_t* darwin_art_bionic_wmemset(wchar_t* destination, wchar_t value,
                                   size_t length);
wchar_t* darwin_art_bionic_wcschr(const wchar_t* string, wchar_t value);
wchar_t* darwin_art_bionic_wcscpy(wchar_t* destination,
                                  const wchar_t* source);

const DarwinArtBionicLeafBinding* darwin_art_bionic_libc_leaf_table(size_t* count);
DarwinArtBionicFunction darwin_art_bionic_libc_leaf_resolve(const char* import_name);

#ifdef __cplusplus
}
#endif

#endif

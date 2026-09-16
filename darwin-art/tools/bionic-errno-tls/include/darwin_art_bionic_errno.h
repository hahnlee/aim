#ifndef DARWIN_ART_BIONIC_ERRNO_H_
#define DARWIN_ART_BIONIC_ERRNO_H_

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*DarwinArtBionicErrnoFunction)(void);

/* Exact Bionic import ABI: returns this pthread's Android-owned errno cell. */
int32_t* darwin_art_bionic___errno(void);

int32_t darwin_art_bionic_errno_load(void);
void darwin_art_bionic_errno_store(int32_t android_errno);

/* Returns one only for a known name-derived mapping. On failure, output and
 * Bionic TLS are unchanged. All functions preserve Darwin host errno. */
int darwin_art_bionic_errno_from_darwin(int darwin_errno,
                                        int32_t* android_errno);
/* Reverse name-derived mapping for host diagnostic boundaries. Preserves both
 * errno cells and leaves output unchanged when no matching host errno exists. */
int darwin_art_bionic_errno_to_darwin(int32_t android_errno, int* darwin_errno);
int darwin_art_bionic_errno_set_from_darwin(int darwin_errno);
int darwin_art_bionic_errno_capture_host(void);

/* Result-seam bridge: zero means success and leaves errno unchanged; a nonzero
 * Android errno is published to this pthread. */
void darwin_art_bionic_errno_publish_result(int32_t android_errno);

/* Closed one-symbol namespace. No host lookup or fallback is performed. */
DarwinArtBionicErrnoFunction darwin_art_bionic_errno_resolve(
    const char* import_name);

#ifdef __cplusplus
}
#endif

#endif

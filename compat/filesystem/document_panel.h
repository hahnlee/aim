#pragma once

#ifdef __cplusplus
extern "C" {
#endif

// Presents the native macOS document picker on the AppKit main thread.
// Returned UTF-8 filesystem paths are malloc-allocated and must be released
// with aim_host_document_path_free(). A null return means cancellation,
// an invalid calling thread, or an unavailable panel URL.
char* aim_host_open_document(const char* mime_type);
char* aim_host_save_document(const char* mime_type,
                                    const char* suggested_name);
void aim_host_document_path_free(char* path);

#ifdef __cplusplus
}  // extern "C"
#endif

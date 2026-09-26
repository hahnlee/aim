// Fixed-argument imports for the unchanged Bionic property client. The client
// owns protocol policy; the existing providers own guest FDs and errno.
#include "darwin_art_bionic_errno.h"
#include "darwin_art_bionic_fs.h"
#include "darwin_art_bionic_libc_leaf.h"
#include "darwin_art_bionic_numeric.h"
#include "darwin_art_bionic_process_state.h"
#include "darwin_art_bionic_socket_broker.h"

// Guest __system_property_set reaches the profile's property service (ADR
// 0009); see darwin-art-runtime property_ffi.rs.
extern int darwin_art_bionic_property_service_set(const char*, const char*);
typedef void (*PropertyFunction)(void);
PropertyFunction darwin_art_bionic_property_client_resolve(const char* name) {
  if (name != NULL && darwin_art_bionic_strcmp(name, "__system_property_set") == 0)
    return (PropertyFunction)darwin_art_bionic_property_service_set;
  return NULL;
}

int32_t* darwin_art_property_import___errno(void) {
  return darwin_art_bionic___errno();
}
int darwin_art_property_import___system_property_get(const char* key, char* value) {
  return darwin_art_bionic___system_property_get(key, value);
}
int darwin_art_property_import_access(const char* path, int mode) {
  return darwin_art_bionic_access(path, mode);
}
int darwin_art_property_import_socket(int domain, int type, int protocol) {
  return darwin_art_bionic_socket_broker_socket(domain, type, protocol);
}
int darwin_art_property_import_connect(int fd, const void* address, uint32_t size) {
  return darwin_art_bionic_socket_broker_connect(fd, address, size);
}
int darwin_art_property_import_close(int fd) {
  return darwin_art_bionic_socket_broker_close(fd);
}
intptr_t darwin_art_property_import_recv(int fd, void* bytes, size_t size, int flags) {
  return darwin_art_bionic_socket_broker_recv(fd, bytes, size, flags);
}
intptr_t darwin_art_property_import_send(int fd, const void* bytes, size_t size,
                                        int flags) {
  return darwin_art_bionic_socket_broker_send(fd, bytes, size, flags);
}
intptr_t darwin_art_property_import_writev(int fd, const struct iovec* vectors,
                                          int count) {
  return darwin_art_bionic_socket_broker_writev(fd, vectors, count);
}
int darwin_art_property_import_poll(DarwinArtBionicPollFd* descriptors,
                                     size_t count, int timeout) {
  return darwin_art_bionic_socket_broker_poll(descriptors, count, timeout);
}
size_t darwin_art_property_import_strlen(const char* value) {
  return darwin_art_bionic_strlen(value);
}
int darwin_art_property_import_strcmp(const char* left, const char* right) {
  return darwin_art_bionic_strcmp(left, right);
}
int darwin_art_property_import_strncmp(const char* left, const char* right,
                                       size_t count) {
  return darwin_art_bionic_strncmp(left, right, count);
}
size_t darwin_art_property_import_strlcpy(char* to, const char* from, size_t size) {
  return darwin_art_bionic_strlcpy(to, from, size);
}
void* darwin_art_property_import_memset(void* to, int value, size_t count) {
  return darwin_art_bionic_memset(to, value, count);
}
long long darwin_art_property_import_atoll(const char* text) {
  return darwin_art_bionic_atoll(text);
}

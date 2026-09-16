#ifndef DARWIN_ART_BIONIC_PROCESS_STATE_H_
#define DARWIN_ART_BIONIC_PROCESS_STATE_H_

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <time.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*DarwinArtBionicProcessFunction)(void);
typedef int (*DarwinArtBionicJitFaultRecovery)(uintptr_t fault_address,
                                                int execution_fault);
typedef int (*DarwinArtBionicSigchainOwnsSignal)(int signal_number);
typedef void (*DarwinArtBionicEnsureFrontOfChain)(int signal_number);

char* darwin_art_bionic_getenv(const char* name);
int darwin_art_bionic_android_get_device_api_level(void);
int darwin_art_bionic_process_state_process_install(void);
/* Read-only: 1 installed, 0 absent, -1 owner failure. No lifetime transfer. */
int darwin_art_bionic_process_state_is_installed(void);
/* Host/service-owned initialization, not an Android app API. Every byte is
 * copied before return; no defaults are fabricated. Call before starting ART,
 * and uninstall only after all process users have quiesced. Returns 0 or -1. */
typedef struct DarwinArtProcessSnapshotEntry {
  const uint8_t* name;
  size_t name_size;
  const uint8_t* value;
  size_t value_size;
} DarwinArtProcessSnapshotEntry;
typedef struct DarwinArtProcessSnapshotConfigV1 {
  uint32_t abi_version; /* 1 */
  uint32_t struct_size;
  const DarwinArtProcessSnapshotEntry* environment;
  size_t environment_count;
  const DarwinArtProcessSnapshotEntry* properties;
  size_t property_count;
  uint64_t page_size;
  uint64_t hwcap;
  uint64_t hwcap2;
  uint8_t secure;
  uint8_t reserved[7]; /* zero */
  uint8_t random[16]; /* supplied by the process launch authority */
} DarwinArtProcessSnapshotConfigV1;
typedef DarwinArtProcessSnapshotConfigV1 DarwinArtProcessSnapshotConfig;
typedef struct DarwinArtProcessCredentialsConfig {
  uint32_t uid;
  uint32_t euid;
  uint32_t suid;
  uint32_t gid;
  uint32_t egid;
  uint32_t sgid;
  const uint32_t* supplementary_groups;
  size_t supplementary_group_count;
  uint64_t permitted;
  uint64_t effective;
  uint64_t inheritable;
} DarwinArtProcessCredentialsConfig;
typedef struct DarwinArtProcessSnapshotConfigV2 {
  DarwinArtProcessSnapshotConfigV1 base;
  DarwinArtProcessCredentialsConfig credentials;
} DarwinArtProcessSnapshotConfigV2;
int darwin_art_bionic_process_state_install_configured(
    const DarwinArtProcessSnapshotConfig* config);
/* Checked readback from the active trusted snapshot. V1 snapshots have no
 * credentials and return -1. On a too-small group buffer, no output field or
 * group is written and the function returns -1 with Android E2BIG. */
typedef struct DarwinArtProcessCredentialsOutput {
  uint32_t uid;
  uint32_t euid;
  uint32_t suid;
  uint32_t gid;
  uint32_t egid;
  uint32_t sgid;
  size_t supplementary_group_count;
  uint64_t permitted;
  uint64_t effective;
  uint64_t inheritable;
} DarwinArtProcessCredentialsOutput;
int darwin_art_bionic_process_state_read_credentials_core(
    DarwinArtProcessCredentialsOutput* output, uint32_t* supplementary_groups,
    size_t group_capacity);
/* Scalar identity readback is independent of supplementary-group capacity. */
int darwin_art_bionic_process_state_read_credential_ids_core(
    DarwinArtProcessCredentialsOutput* output);
int darwin_art_bionic_process_state_process_uninstall(void);
void darwin_art_bionic_process_state_bind_jit_fault_recovery(
    DarwinArtBionicJitFaultRecovery recovery);
int darwin_art_bionic_process_state_recover_runtime_signal(
    int host_signal, void* host_info, void* host_context);
void darwin_art_bionic_process_state_bind_sigchain(
    DarwinArtBionicSigchainOwnsSignal owns_signal,
    DarwinArtBionicEnsureFrontOfChain ensure_front);
int darwin_art_bionic___system_property_get(const char* name, char* value);
const void* darwin_art_bionic___system_property_find(const char* name);
typedef void (*DarwinArtBionicPropertyForeachCallback)(const void* property,
                                                        void* cookie);
int darwin_art_bionic___system_property_foreach(
    DarwinArtBionicPropertyForeachCallback callback, void* cookie);
uint32_t darwin_art_bionic___system_property_serial(const void* property);
uint32_t darwin_art_bionic___system_property_area_serial(void);
uint32_t darwin_art_bionic_process_property_serial_core(const void* property);
uint32_t darwin_art_bionic_process_property_area_serial_core(void);
typedef struct DarwinArtPropertyWaitTimeout {
  int64_t seconds;
  int64_t nanoseconds;
} DarwinArtPropertyWaitTimeout;
int darwin_art_bionic_process_property_wait_core(const void*, uint32_t, uint32_t*,
                                                const DarwinArtPropertyWaitTimeout*);
bool darwin_art_bionic___system_property_wait(const void*, uint32_t, uint32_t*,
                                            const struct timespec*);
int darwin_art_bionic___system_property_read(const void* property, char* name,
                                             char* value);
void darwin_art_bionic___system_property_read_callback(
    const void* property,
    void (*callback)(void*, const char*, const char*, uint32_t), void* cookie);
unsigned long darwin_art_bionic_getauxval(unsigned long type);
char* darwin_art_bionic_basename(const char* path);
int darwin_art_bionic_getentropy(void* buffer, size_t length);
int darwin_art_bionic_rand(void);
void darwin_art_bionic_srand(unsigned seed);
long darwin_art_bionic_random(void);
void darwin_art_bionic_srandom(unsigned seed);
int darwin_art_bionic_rand_r(unsigned* seed);
double darwin_art_bionic_erand48(unsigned short state[3]);
long darwin_art_bionic_nrand48(unsigned short state[3]);
long darwin_art_bionic_jrand48(unsigned short state[3]);
void darwin_art_bionic_srand48(long seed);
double darwin_art_bionic_drand48(void);
long darwin_art_bionic_lrand48(void);
long darwin_art_bionic_mrand48(void);
uint32_t darwin_art_bionic_arc4random(void);
int darwin_art_bionic_getpid(void);
int darwin_art_bionic_kill(int pid, int signal_number);
unsigned darwin_art_bionic_geteuid(void);
int darwin_art_bionic_getpagesize(void);
int darwin_art_bionic_daemon(int nochdir, int noclose);
int darwin_art_bionic_posix_spawn(int* process_id, const char* path,
                                  const void* file_actions,
                                  const void* attributes, char* const argv[],
                                  char* const environment[]);
int darwin_art_bionic_setjmp(void* environment);
void darwin_art_bionic_longjmp(void* environment, int value);
DarwinArtBionicProcessFunction darwin_art_bionic_process_state_resolve(
    const char* name);
uintptr_t darwin_art_bionic_process_state_data_resolve(const char* name);

char* darwin_art_bionic_process_getenv_core(const char* name);
int darwin_art_bionic_process_property_get_core(const char* name, char* value);
const void* darwin_art_bionic_process_property_find_core(const char* name);
void darwin_art_bionic_process_property_read_callback_core(
    const void* property,
    void (*callback)(void*, const char*, const char*, uint32_t), void* cookie);
unsigned long darwin_art_bionic_process_getauxval_core(unsigned long type);

#ifdef __cplusplus
}
#endif

#endif

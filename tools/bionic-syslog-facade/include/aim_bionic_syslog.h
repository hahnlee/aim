#ifndef AIM_BIONIC_SYSLOG_H_
#define AIM_BIONIC_SYSLOG_H_

#ifdef __cplusplus
extern "C" {
#endif

typedef void (*AimBionicSyslogFunction)(void);

void aim_bionic_openlog(const char* ident, int option, int facility);
void aim_bionic_closelog(void);
void aim_bionic_syslog(int priority, const char* format, ...);

/* One-time owned copy of the guest process tag used when openlog has no tag. */
int aim_bionic_syslog_activate(const char* guest_program_tag);

AimBionicSyslogFunction aim_bionic_syslog_resolve(
    const char* soname, const char* symbol, const char* version);
const char* aim_bionic_syslog_capability(const char* capability);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // AIM_BIONIC_SYSLOG_H_

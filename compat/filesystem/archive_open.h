#pragma once
using AimArchiveOpener = int (*)(const char*);
void aim_set_archive_opener(AimArchiveOpener opener);
int aim_archive_open(const char* path, int flags, ...);
void aim_install_archive_filesystem(bool enabled);
// Status of a guest path: file type, permission bits and size, as the
// process's Android filesystem namespace reports them. Returns false (errno
// set) when no guest filesystem is installed or the path does not exist.
struct stat;
using AimArchiveStat = bool (*)(const char*, struct stat*);
void aim_set_archive_stat(AimArchiveStat stat);
bool aim_archive_stat(const char* path, struct stat* status);

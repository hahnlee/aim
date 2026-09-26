#pragma once
using DarwinArtArchiveOpener = int (*)(const char*);
void darwin_art_set_archive_opener(DarwinArtArchiveOpener opener);
int darwin_art_archive_open(const char* path, int flags, ...);
void darwin_art_install_archive_filesystem(bool enabled);
// Status of a guest path: file type, permission bits and size, as the
// process's Android filesystem namespace reports them. Returns false (errno
// set) when no guest filesystem is installed or the path does not exist.
struct stat;
using DarwinArtArchiveStat = bool (*)(const char*, struct stat*);
void darwin_art_set_archive_stat(DarwinArtArchiveStat stat);
bool darwin_art_archive_stat(const char* path, struct stat* status);

#pragma once
using DarwinArtArchiveOpener = int (*)(const char*);
void darwin_art_set_archive_opener(DarwinArtArchiveOpener opener);
int darwin_art_archive_open(const char* path, int flags, ...);
void darwin_art_install_archive_filesystem(bool enabled);

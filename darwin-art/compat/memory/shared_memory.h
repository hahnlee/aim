#pragma once

#include <cstddef>

extern "C" int ASharedMemory_create(const char* name, size_t size);
extern "C" int ASharedMemory_setProt(int fd, int protection);
extern "C" int darwin_art_android_shared_memory_close(int fd);
extern "C" int darwin_art_android_shared_memory_dup(int fd);

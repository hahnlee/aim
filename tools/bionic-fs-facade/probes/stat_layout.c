#include <stddef.h>
#include <dirent.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <unistd.h>

_Static_assert(sizeof(struct stat) == 128, "Android arm64 stat size drift");
_Static_assert(offsetof(struct stat, st_dev) == 0, "st_dev offset drift");
_Static_assert(offsetof(struct stat, st_ino) == 8, "st_ino offset drift");
_Static_assert(offsetof(struct stat, st_mode) == 16, "st_mode offset drift");
_Static_assert(offsetof(struct stat, st_nlink) == 20, "st_nlink offset drift");
_Static_assert(offsetof(struct stat, st_uid) == 24, "st_uid offset drift");
_Static_assert(offsetof(struct stat, st_gid) == 28, "st_gid offset drift");
_Static_assert(offsetof(struct stat, st_rdev) == 32, "st_rdev offset drift");
_Static_assert(offsetof(struct stat, st_size) == 48, "st_size offset drift");
_Static_assert(offsetof(struct stat, st_blksize) == 56, "st_blksize offset drift");
_Static_assert(offsetof(struct stat, st_blocks) == 64, "st_blocks offset drift");
_Static_assert(offsetof(struct stat, st_atim) == 72, "st_atim offset drift");
_Static_assert(offsetof(struct stat, st_mtim) == 88, "st_mtim offset drift");
_Static_assert(offsetof(struct stat, st_ctim) == 104, "st_ctim offset drift");
_Static_assert(sizeof(struct dirent) == 280, "Android arm64 dirent size drift");
_Static_assert(sizeof(struct dirent64) == 280,
               "Android arm64 dirent64 size drift");
_Static_assert(offsetof(struct dirent, d_ino) == 0, "dirent ino offset drift");
_Static_assert(offsetof(struct dirent, d_off) == 8, "dirent off offset drift");
_Static_assert(offsetof(struct dirent, d_reclen) == 16,
               "dirent reclen offset drift");
_Static_assert(offsetof(struct dirent, d_type) == 18,
               "dirent type offset drift");
_Static_assert(offsetof(struct dirent, d_name) == 19,
               "dirent name offset drift");
_Static_assert(sizeof(((struct dirent*)0)->d_name) == 256,
               "dirent name capacity drift");
_Static_assert(sizeof(struct statvfs) == 112,
               "Android arm64 statvfs size drift");
_Static_assert(offsetof(struct statvfs, f_bsize) == 0,
               "statvfs block size offset drift");
_Static_assert(offsetof(struct statvfs, f_flag) == 72,
               "statvfs flags offset drift");
_Static_assert(offsetof(struct statvfs, f_namemax) == 80,
               "statvfs name max offset drift");
_Static_assert(offsetof(struct statvfs, __f_reserved) == 88,
               "statvfs reserved offset drift");
_Static_assert(DT_UNKNOWN == 0 && DT_FIFO == 1 && DT_CHR == 2 && DT_DIR == 4 &&
                   DT_BLK == 6 && DT_REG == 8 && DT_LNK == 10 &&
                   DT_SOCK == 12 && DT_WHT == 14,
               "Android directory type values drift");

_Static_assert(O_RDONLY == 0 && O_WRONLY == 1 && O_RDWR == 2,
               "Android access mode drift");
_Static_assert(O_CREAT == 64 && O_TRUNC == 512 && O_APPEND == 1024,
               "Android write flag drift");
_Static_assert(O_NONBLOCK == 2048 && O_DSYNC == 4096,
               "Android status flag drift");
_Static_assert(O_DIRECTORY == 16384 && O_NOFOLLOW == 32768 && O_DIRECT == 65536,
               "Android arm64 lookup flag drift");
_Static_assert(O_LARGEFILE == 131072 && O_CLOEXEC == 524288,
               "Android arm64 descriptor flag drift");
_Static_assert(O_SYNC == 1052672 && O_PATH == 2097152 && O_TMPFILE == 4210688,
               "Android extended flag drift");
_Static_assert(AT_FDCWD == -100 && AT_SYMLINK_NOFOLLOW == 0x100 &&
                   AT_REMOVEDIR == 0x200 && AT_EMPTY_PATH == 0x1000,
               "Android at flag drift");
_Static_assert(_PC_FILESIZEBITS == 0 && _PC_LINK_MAX == 1 &&
                   _PC_NAME_MAX == 4 && _PC_PATH_MAX == 5 &&
                   _PC_2_SYMLINKS == 7 && _PC_ALLOC_SIZE_MIN == 8 &&
                   _PC_SYMLINK_MAX == 13 && _PC_CHOWN_RESTRICTED == 14 &&
                   _PC_SYNC_IO == 19,
               "Android pathconf selector drift");
_Static_assert(ST_RDONLY == 0x0001 && ST_NOSUID == 0x0002 &&
                   ST_NODEV == 0x0004 && ST_NOEXEC == 0x0008 &&
                   ST_NOSYMFOLLOW == 0x2000,
               "Android statvfs flag drift");

typedef int (*OpenSignature)(const char*, int, ...);
typedef int (*OpenatSignature)(int, const char*, int, ...);
typedef ssize_t (*ReadSignature)(int, void*, size_t);
typedef int (*CloseSignature)(int);
typedef int (*FstatSignature)(int, struct stat*);
typedef int (*StatSignature)(const char*, struct stat*);
typedef ssize_t (*ReadlinkSignature)(const char*, char*, size_t);
typedef char* (*GetcwdSignature)(char*, size_t);
typedef int (*ChdirSignature)(const char*);
typedef DIR* (*OpendirSignature)(const char*);
typedef struct dirent* (*ReaddirSignature)(DIR*);
typedef int (*ClosedirSignature)(DIR*);
typedef int (*FchmodSignature)(int, mode_t);
typedef int (*FchmodatSignature)(int, const char*, mode_t, int);
typedef int (*FtruncateSignature)(int, off_t);
typedef int (*IsattySignature)(int);
typedef int (*LinkSignature)(const char*, const char*);
typedef int (*MkdirSignature)(const char*, mode_t);
typedef long (*PathconfSignature)(const char*, int);
typedef char* (*RealpathSignature)(const char*, char*);
typedef int (*OnePathSignature)(const char*);
typedef int (*RenameSignature)(const char*, const char*);
typedef int (*StatvfsSignature)(const char*, struct statvfs*);
typedef int (*TruncateSignature)(const char*, off_t);
typedef int (*UnlinkatSignature)(int, const char*, int);
typedef int (*UtimensatSignature)(int, const char*, const struct timespec*, int);
_Static_assert(_Generic(&open, OpenSignature: 1, default: 0), "open signature drift");
_Static_assert(_Generic(&openat, OpenatSignature: 1, default: 0),
               "openat signature drift");
_Static_assert(_Generic(&read, ReadSignature: 1, default: 0), "read signature drift");
_Static_assert(_Generic(&close, CloseSignature: 1, default: 0), "close signature drift");
_Static_assert(_Generic(&fstat, FstatSignature: 1, default: 0), "fstat signature drift");
_Static_assert(_Generic(&stat, StatSignature: 1, default: 0), "stat signature drift");
_Static_assert(_Generic(&lstat, StatSignature: 1, default: 0), "lstat signature drift");
_Static_assert(_Generic(&readlink, ReadlinkSignature: 1, default: 0),
               "readlink signature drift");
_Static_assert(_Generic(&getcwd, GetcwdSignature: 1, default: 0),
               "getcwd signature drift");
_Static_assert(_Generic(&chdir, ChdirSignature: 1, default: 0),
               "chdir signature drift");
_Static_assert(_Generic(&opendir, OpendirSignature: 1, default: 0),
               "opendir signature drift");
_Static_assert(_Generic(&readdir, ReaddirSignature: 1, default: 0),
               "readdir signature drift");
_Static_assert(_Generic(&closedir, ClosedirSignature: 1, default: 0),
               "closedir signature drift");
_Static_assert(_Generic(&fchmod, FchmodSignature: 1, default: 0),
               "fchmod signature drift");
_Static_assert(_Generic(&fchmodat, FchmodatSignature: 1, default: 0),
               "fchmodat signature drift");
_Static_assert(_Generic(&ftruncate, FtruncateSignature: 1, default: 0),
               "ftruncate signature drift");
_Static_assert(_Generic(&isatty, IsattySignature: 1, default: 0),
               "isatty signature drift");
_Static_assert(_Generic(&link, LinkSignature: 1, default: 0),
               "link signature drift");
_Static_assert(_Generic(&mkdir, MkdirSignature: 1, default: 0),
               "mkdir signature drift");
_Static_assert(_Generic(&pathconf, PathconfSignature: 1, default: 0),
               "pathconf signature drift");
_Static_assert(_Generic(&realpath, RealpathSignature: 1, default: 0),
               "realpath signature drift");
_Static_assert(_Generic(&remove, OnePathSignature: 1, default: 0),
               "remove signature drift");
_Static_assert(_Generic(&rename, RenameSignature: 1, default: 0),
               "rename signature drift");
_Static_assert(_Generic(&statvfs, StatvfsSignature: 1, default: 0),
               "statvfs signature drift");
_Static_assert(_Generic(&symlink, LinkSignature: 1, default: 0),
               "symlink signature drift");
_Static_assert(_Generic(&truncate, TruncateSignature: 1, default: 0),
               "truncate signature drift");
_Static_assert(_Generic(&unlinkat, UnlinkatSignature: 1, default: 0),
               "unlinkat signature drift");
_Static_assert(_Generic(&utimensat, UtimensatSignature: 1, default: 0),
               "utimensat signature drift");

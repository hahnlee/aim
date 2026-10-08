// Compile-only original SDK API; implementation remains in the original image.
package android.system; public final class StructStat {public int st_uid;public int st_gid;public int st_mode; public long st_ino;
 public StructStat(long dev,long ino,int mode,long nlink,int uid,int gid,long rdev,long size,long atime,long mtime,long ctime,long blksize,long blocks){throw new RuntimeException("stub");}
}

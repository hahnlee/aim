// Compile-only original SDK API; implementation remains in the original image.
package android.os.storage;
public final class VolumeInfo {
 public static int TYPE_PRIVATE=0;
 public int getType(){throw new RuntimeException("stub");} public boolean isMountedReadable(){throw new RuntimeException("stub");} public String getFsUuid(){throw new RuntimeException("stub");}
 public static final int STATE_MOUNTED=2;
 public java.io.File getPath(){throw new RuntimeException("stub");}public boolean isMountedWritable(){throw new RuntimeException("stub");}
 public VolumeInfo(android.os.Parcel parcel){throw new RuntimeException("stub");}
}

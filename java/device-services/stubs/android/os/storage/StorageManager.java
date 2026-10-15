// Compile-only members checked against the original image.
package android.os.storage;
public class StorageManager {
    public void setPrimaryStorageUuid(String volumeUuid, android.content.pm.IPackageMoveObserver callback) { throw new RuntimeException("stub"); }
    public StorageManager(android.content.Context context, android.os.Looper looper) { throw new RuntimeException("stub"); }
    public void allocateBytes(java.io.FileDescriptor file, long bytes, int flags) throws java.io.IOException { throw new RuntimeException("stub"); }
    public static boolean isCeStorageUnlocked(int userId) {throw new RuntimeException("stub");}
    public java.util.List<VolumeInfo> getVolumes(){throw new RuntimeException("stub");}
 public static final int FLAG_STORAGE_DE=1,FLAG_STORAGE_CE=2;
 public void registerListener(StorageEventListener listener){throw new RuntimeException("stub");}
 public void unregisterListener(StorageEventListener listener){throw new RuntimeException("stub");}
 public java.io.File findPathForUuid(String volume)throws java.io.IOException{throw new RuntimeException("stub");}
 public static final String UUID_PRIVATE_INTERNAL=null;
 public java.util.List<VolumeInfo> getWritablePrivateVolumes(){throw new RuntimeException("stub");}
 public static boolean isFileEncrypted(){throw new RuntimeException("stub");}
 public VolumeInfo getPrimaryPhysicalVolume(){throw new RuntimeException("stub");}
 public VolumeInfo findVolumeByUuid(String volume){throw new RuntimeException("stub");}
 public long getStorageBytesUntilLow(java.io.File path){throw new RuntimeException("stub");}
 public static final String UUID_PRIMARY_PHYSICAL="primary_physical";
}

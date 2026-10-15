// Compile-only pinned original type; native subclass overrides every PMS-dependent entry.
package com.android.server.pm;
public class PackageArchiver {
    PackageArchiver(android.content.Context context, PackageManagerService service) { throw new RuntimeException("stub"); }
    void requestArchive(String name,String caller,android.content.IntentSender sender,android.os.UserHandle user){throw new RuntimeException("stub");}
    void requestArchive(String name,String caller,int flags,android.content.IntentSender sender,android.os.UserHandle user){throw new RuntimeException("stub");}
    void requestUnarchive(String name,String caller,android.content.IntentSender sender,android.os.UserHandle user){throw new RuntimeException("stub");}
    public int requestUnarchiveOnActivityStart(android.content.Intent intent,String caller,int user,int uid){throw new RuntimeException("stub");}
    public boolean isIntentResolvedToArchivedApp(android.content.Intent intent,int user){throw new RuntimeException("stub");}
    void clearArchiveState(String name,int user){throw new RuntimeException("stub");}
    void clearArchiveState(PackageSetting setting,int user){throw new RuntimeException("stub");}
    com.android.server.pm.pkg.ArchiveState createArchiveState(android.content.pm.ArchivedPackageParcel parcel,int user,String installer,String title){throw new RuntimeException("stub");}
    public boolean verifySupportsUnarchival(String name,int user){throw new RuntimeException("stub");}
    public boolean isAppArchivable(String name,android.os.UserHandle user){throw new RuntimeException("stub");}
    public android.graphics.Bitmap getArchivedAppIcon(String name,android.os.UserHandle user,String caller){throw new RuntimeException("stub");}
    void notifyUnarchivalListener(int status,String installer,String name,long bytes,android.app.PendingIntent action,java.util.Set<android.content.IntentSender> senders,int user){throw new RuntimeException("stub");}
}

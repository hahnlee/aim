package com.android.server.pm;
import android.os.Parcel;
import android.system.Os;
import android.system.StructStat;
import android.util.AtomicFile;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.UUID;

/** Observe original creators' real uid/gid/umask instead of guessing metadata. */
public final class NativeBootInodeInputs {
    public static byte[] capture(String requested) throws IOException, android.system.ErrnoException {
        boolean stage = "/data/app/vmdl.session.tmp".equals(requested);
        boolean sessions = "/data/system/install_sessions.xml".equals(requested);
        boolean runtime = requested != null && requested.matches(
                "/data/misc_de/[0-9]+/apexdata/com.android.permission/runtime-permissions.xml");
        if (!stage && !sessions && !runtime) throw new IllegalArgumentException("Unowned inode request");
        File parent = stage ? new File("/data/app") : new File(requested).getParentFile();
        if (!parent.isDirectory()) throw new IOException("Creation parent owner absent: " + parent);
        File probe = new File(parent, "vmdl." + UUID.randomUUID() + ".aim-config.tmp");
        StructStat stat;
        if (stage) {
            try { PackageInstallerService.prepareStageDir(probe); stat = Os.stat(probe.getPath()); }
            finally { if (probe.exists() && !probe.delete()) throw new IOException("Cannot remove owned stage probe"); }
        } else {
            AtomicFile atomic = new AtomicFile(probe);
            FileOutputStream stream = null;
            try { stream = atomic.startWrite(); stat = Os.fstat(stream.getFD()); }
            finally {
                if (stream != null) atomic.failWrite(stream);
                for (String suffix : new String[]{"", ".new", ".bak"}) {
                    File owned = new File(probe.getPath() + suffix);
                    if (owned.exists() && !owned.delete()) throw new IOException("Cannot remove owned AtomicFile probe");
                }
            }
        }
        Parcel parcel = Parcel.obtain();
        try { parcel.writeInt(stat.st_uid); parcel.writeInt(stat.st_gid); parcel.writeInt(stat.st_mode & 07777); return parcel.marshall(); }
        finally { parcel.recycle(); }
    }
}

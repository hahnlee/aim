import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.system.Os;
import java.io.FileDescriptor;

/** Explicit true-mode native test owner; the image's installer mode remains unchanged. */
public final class InstallerProxyOracle {
    private static Parcel request(String descriptor) {
        Parcel p = Parcel.obtain(); p.writeInterfaceToken(descriptor); return p;
    }
    private static Parcel call(IBinder binder, int code, Parcel request) throws Exception {
        Parcel reply = Parcel.obtain();
        if (!binder.transact(code, request, reply, 0)) throw new AssertionError("unhandled transaction " + code);
        request.recycle(); reply.readException(); return reply;
    }
    private static ParcelFileDescriptor transfer(ParcelFileDescriptor fd) throws Exception {
        FileDescriptor send = new FileDescriptor(), receive = new FileDescriptor();
        // Linux UAPI: AF_UNIX=1, SOCK_DGRAM=2, SOL_SOCKET=1, SCM_RIGHTS=1.
        Os.socketpair(1, 2, 0, send, receive);
        try {
            byte[] rights = java.nio.ByteBuffer.allocate(4).order(java.nio.ByteOrder.nativeOrder()).putInt(fd.getFd()).array();
            var outgoing = new android.system.StructMsghdr(null,
                new java.nio.ByteBuffer[] { java.nio.ByteBuffer.wrap(new byte[] { 73 }) },
                new android.system.StructCmsghdr[] { new android.system.StructCmsghdr(1, 1, rights) }, 0);
            if (Os.sendmsg(send, outgoing, 0) != 1) throw new AssertionError("SCM send changed");
            var payload = java.nio.ByteBuffer.allocate(1);
            var incoming = new android.system.StructMsghdr(null,
                new java.nio.ByteBuffer[] { payload },
                new android.system.StructCmsghdr[] { new android.system.StructCmsghdr(1, 1, new byte[4]) }, 0);
            if (Os.recvmsg(receive, incoming, 0) != 1 || payload.get(0) != 73 || incoming.msg_flags != 0
                    || incoming.msg_control.length != 1) throw new AssertionError("SCM receive changed");
            var control = incoming.msg_control[0];
            if (control.cmsg_level != 1 || control.cmsg_type != 1 || control.cmsg_data.length != 4)
                throw new AssertionError("SCM control changed");
            return ParcelFileDescriptor.adoptFd(java.nio.ByteBuffer.wrap(control.cmsg_data)
                .order(java.nio.ByteOrder.nativeOrder()).getInt());
        } finally { Os.close(send); Os.close(receive); }
    }
    private static void rejected(FileDescriptor fd) throws Exception {
        try { Os.write(fd, new byte[] { 1 }, 0, 1); throw new AssertionError("revoked write accepted"); }
        catch (android.system.ErrnoException expected) { if (expected.errno != 1) throw expected; }
    }
    private static IBinder openSession(IBinder installer, String[] args, int id) throws Exception {
        Parcel p = request("android.content.pm.IPackageInstaller"); p.writeInt(id);
        Parcel reply = call(installer, Integer.parseInt(args[1]), p);
        IBinder session = reply.readStrongBinder(); reply.recycle(); return session;
    }
    private static void afterExec(IBinder installer, String[] args) throws Exception {
        try (var inherited = ParcelFileDescriptor.adoptFd(Integer.parseInt(args[7]))) {
            FileDescriptor fd = inherited.getFileDescriptor();
            if (Os.fstat(fd).st_size != 1 || Os.lseek(fd, 0, 1) != 1) throw new AssertionError("exec lost proxy state");
            if (Os.write(fd, new byte[] { 2, 3 }, 0, 2) != 2) throw new AssertionError("exec proxy write failed");
            Os.fsync(fd);
            if (Os.fstat(fd).st_size != 3) throw new AssertionError("exec proxy size changed");
            IBinder session = openSession(installer, args, Integer.parseInt(args[6]));
            Parcel reply = call(session, Integer.parseInt(args[3]), request("android.content.pm.IPackageInstallerSession")); reply.recycle();
            rejected(fd);
        }
        System.out.println("PROXY original SCM_RIGHTS retransmit exec classification revoke");
    }
    public static void main(String[] args) {
        try {
            if (args.length == 4 && "permission-check".equals(args[0])) {
                if (android.os.Process.myUid() != 1000 || android.os.Binder.getCallingUid() != 1000
                        || android.os.Binder.getCallingPid() != android.os.Process.myPid()) {
                    throw new IllegalStateException("original permission caller credentials differ");
                }
                var activity = android.app.ActivityManager.getService();
                if (activity == null) throw new IllegalStateException("original activity permission owner unavailable");
                int status = activity.checkPermission(args[1], Integer.parseInt(args[2]), Integer.parseInt(args[3]));
                if (status != 0 && status != -1) throw new IllegalStateException("invalid original permission status " + status);
                System.out.println("PERMISSION_RESULT " + status);
                return;
            }
            if (android.os.Process.myUid() != 10100 || android.os.Binder.getCallingUid() != 10100
                    || android.os.Binder.getCallingPid() != android.os.Process.myPid()) {
                throw new IllegalStateException("original app caller credentials differ");
            }
            String installerDescriptor = "android.content.pm.IPackageInstaller";
            String sessionDescriptor = "android.content.pm.IPackageInstallerSession";
            IBinder installer = com.android.internal.os.BinderInternal.getContextObject();
            if (installer == null) throw new AssertionError("native fixture owner missing");
            if (args.length > 5) { afterExec(installer, args); System.exit(0); }
            Parcel p = request(installerDescriptor);
            var params = new android.content.pm.PackageInstaller.SessionParams(1);
            params.sizeBytes = -1; params.appPackageName = "fixture";
            p.writeTypedObject(params, 0); p.writeString("fixture"); p.writeString(null); p.writeInt(0);
            Parcel reply = call(installer, Integer.parseInt(args[0]), p);
            int id = reply.readInt(); reply.recycle();
            p = request(installerDescriptor); p.writeInt(id);
            reply = call(installer, Integer.parseInt(args[1]), p);
            IBinder session = reply.readStrongBinder(); reply.recycle();
            p = request(sessionDescriptor); p.writeString("base.apk"); p.writeLong(0); p.writeLong(-1);
            reply = call(session, Integer.parseInt(args[2]), p);
            ParcelFileDescriptor target = reply.readTypedObject(ParcelFileDescriptor.CREATOR); reply.recycle();
            if (target == null) throw new AssertionError("native proxy missing");
            FileDescriptor duplicate = Os.dup(target.getFileDescriptor());
            try (var first = transfer(target); var second = transfer(first);
                    var output = new ParcelFileDescriptor.AutoCloseOutputStream(target)) {
                byte[] bytes = new byte[17003];
                for (int i = 0; i < bytes.length; i++) bytes[i] = (byte)(i * 31 + 7);
                output.write(bytes); Os.fsync(duplicate);
                if (Os.lseek(duplicate, 0, 1) != bytes.length) throw new AssertionError("dup offset not shared");
                if (Os.lseek(duplicate, 73, 0) != 73) throw new AssertionError("seek failed");
                output.write(new byte[] { 99, 98, 97 }); Os.fsync(duplicate);
                if (Os.lseek(duplicate, 0, 1) != 76) throw new AssertionError("stream did not share dup seek");
                if (Os.lseek(second.getFileDescriptor(), 0, 1) != 76) throw new AssertionError("SCM retransmit lost offset");
                Os.fsync(second.getFileDescriptor());
                if (Os.fstat(duplicate).st_size != bytes.length) throw new AssertionError("size changed");
                bytes[73] = 99; bytes[74] = 98; bytes[75] = 97;
                byte[] actual = new byte[bytes.length];
                try { Os.pread(duplicate, actual, 0, actual.length, 0); throw new AssertionError("write-only fd readable"); }
                catch (android.system.ErrnoException expected) { if (expected.errno != 9) throw expected; }
                p = request(sessionDescriptor); p.writeString("base.apk");
                reply = call(session, Integer.parseInt(args[4]), p);
                ParcelFileDescriptor readback = reply.readTypedObject(ParcelFileDescriptor.CREATOR); reply.recycle();
                try (var input = new ParcelFileDescriptor.AutoCloseInputStream(readback)) {
                    if (!java.util.Arrays.equals(input.readAllBytes(), bytes)) throw new AssertionError("proxy readback changed");
                }
                p = request(sessionDescriptor);
                reply = call(session, Integer.parseInt(args[3]), p); reply.recycle();
                for (FileDescriptor fd : new FileDescriptor[] { target.getFileDescriptor(), duplicate,
                        first.getFileDescriptor(), second.getFileDescriptor() }) rejected(fd);
            } finally { Os.close(duplicate); }
            System.out.println("PROXY original AutoCloseOutputStream Binder typed fd dup seek fsync fstat revoke");
            p = request(installerDescriptor); p.writeTypedObject(params, 0);
            p.writeString("fixture"); p.writeString(null); p.writeInt(0);
            reply = call(installer, Integer.parseInt(args[0]), p); int execId = reply.readInt(); reply.recycle();
            session = openSession(installer, args, execId);
            p = request(sessionDescriptor); p.writeString("base.apk"); p.writeLong(0); p.writeLong(-1);
            reply = call(session, Integer.parseInt(args[2]), p);
            try (var fresh = reply.readTypedObject(ParcelFileDescriptor.CREATOR); var inherited = transfer(fresh)) {
                reply.recycle();
                Os.write(inherited.getFileDescriptor(), new byte[] { 1 }, 0, 1);
                // Linux F_SETFD=2: explicitly inherit this real capability across exec.
                Os.fcntlInt(inherited.getFileDescriptor(), 2, 0);
                int raw = inherited.detachFd();
                Os.execv("/system/bin/app_process", new String[] { "/system/bin/app_process",
                    "-Djava.class.path=/data/local/tmp/proxy-oracle.dex", "/system/bin", "InstallerProxyOracle",
                    args[0], args[1], args[2], args[3], args[4], "after-exec", Integer.toString(execId), Integer.toString(raw) });
                throw new AssertionError("exec returned");
            }
        } catch (Throwable failure) { failure.printStackTrace(System.out); System.exit(1); }
    }
}

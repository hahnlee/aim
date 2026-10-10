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
    private static byte[] permissionFrame(java.io.DataInputStream in) throws Exception {
        int length = in.readInt();
        if (length < 10 || length > 65536) throw new IllegalArgumentException("permission frame length");
        byte[] bytes = new byte[length]; in.readFully(bytes); return bytes;
    }
    private static String permissionString(java.io.DataInputStream in) throws Exception {
        int length = in.readInt();
        if (length < 0 || length > 60000 || length > in.available()) throw new IllegalArgumentException("permission string length");
        byte[] bytes = new byte[length]; in.readFully(bytes);
        return java.nio.charset.StandardCharsets.UTF_8.newDecoder()
            .onMalformedInput(java.nio.charset.CodingErrorAction.REPORT)
            .onUnmappableCharacter(java.nio.charset.CodingErrorAction.REPORT)
            .decode(java.nio.ByteBuffer.wrap(bytes)).toString();
    }
    private static void permissionString(java.io.DataOutputStream out, String text) throws Exception {
        byte[] bytes = text.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        if (bytes.length > 60000) throw new IllegalArgumentException("permission string length");
        out.writeInt(bytes.length); out.write(bytes);
    }
    private static void permissionReply(java.io.DataOutputStream out, byte[] bytes) throws Exception {
        if (bytes.length > 65536) throw new IllegalArgumentException("permission reply length");
        out.writeInt(bytes.length); out.write(bytes); out.flush();
        if (System.out.checkError()) throw new IllegalStateException("permission protocol stdout failed");
    }
    private static void permissionIdentity(int pid) {
        if (pid <= 0 || android.os.Process.myPid() != pid || android.os.Process.myUid() != 1000
                || android.os.Binder.getCallingUid() != 1000 || android.os.Binder.getCallingPid() != pid)
            throw new IllegalStateException("original permission caller credentials differ");
    }
    private static void permissionServer() throws Exception {
        System.err.println("PERMISSION_ENTERED_MAIN"); System.err.flush();
        if (System.err.checkError()) throw new IllegalStateException("permission entry stderr failed");
        int pid = android.os.Process.myPid(); permissionIdentity(pid);
        if (android.app.ActivityManager.getService() == null) throw new IllegalStateException("original activity permission owner unavailable");
        var input = new java.io.DataInputStream(System.in);
        var output = new java.io.DataOutputStream(System.out);
        var bytes = new java.io.ByteArrayOutputStream();
        var ready = new java.io.DataOutputStream(bytes);
        ready.writeByte(1); ready.writeByte(1); ready.writeLong(0);
        ready.writeInt(pid); ready.writeInt(1000); ready.writeInt(android.os.Binder.getCallingPid()); ready.writeInt(android.os.Binder.getCallingUid());
        permissionReply(output, bytes.toByteArray());
        long expected = 1;
        for (;;) {
            var frame = new java.io.DataInputStream(new java.io.ByteArrayInputStream(permissionFrame(input)));
            if (frame.readUnsignedByte() != 1) throw new IllegalArgumentException("permission protocol version");
            int kind = frame.readUnsignedByte(); long sequence = frame.readLong();
            if (sequence != expected || sequence <= 0) throw new IllegalArgumentException("permission sequence");
            permissionIdentity(pid);
            bytes.reset(); var reply = new java.io.DataOutputStream(bytes);
            if (kind == 4) {
                if (frame.available() != 0) throw new IllegalArgumentException("shutdown tail");
                reply.writeByte(1); reply.writeByte(5); reply.writeLong(sequence); reply.writeInt(pid); reply.writeInt(1000);
                permissionReply(output, bytes.toByteArray()); return;
            }
            if (kind != 2) throw new IllegalArgumentException("permission request kind");
            String permission = permissionString(frame); int targetPid = frame.readInt(); int targetUid = frame.readInt();
            if (frame.available() != 0) throw new IllegalArgumentException("permission request tail");
            try {
                System.err.println("PERMISSION_REQUEST sequence=" + sequence);
                System.err.println("PERMISSION_PHASE main pid=" + pid + " uid=" + android.os.Process.myUid());
                System.err.println("PERMISSION_PHASE getService pid=" + pid + " uid=" + android.os.Process.myUid());
                System.err.flush();
                var activity = android.app.ActivityManager.getService();
                if (activity == null) throw new IllegalStateException("original activity permission owner unavailable");
                System.err.println("PERMISSION_PHASE checkPermission pid=" + pid + " uid=" + android.os.Process.myUid()); System.err.flush();
                int status = activity.checkPermission(permission, targetPid, targetUid);
                if (status != 0 && status != -1) throw new IllegalStateException("invalid original permission status " + status);
                System.err.println("PERMISSION_PHASE reply pid=" + pid + " uid=" + android.os.Process.myUid() + " status=" + status); System.err.flush();
                if (System.err.checkError()) throw new IllegalStateException("permission phase stderr failed");
                reply.writeByte(1); reply.writeByte(3); reply.writeLong(sequence); reply.writeInt(pid); reply.writeInt(1000);
                permissionString(reply, permission); reply.writeInt(targetPid); reply.writeInt(targetUid); reply.writeInt(status);
                permissionReply(output, bytes.toByteArray()); expected++;
            } catch (Throwable failure) {
                bytes.reset(); reply = new java.io.DataOutputStream(bytes);
                reply.writeByte(1); reply.writeByte(6); reply.writeLong(sequence); permissionString(reply, failure.toString());
                permissionReply(output, bytes.toByteArray()); throw failure;
            }
        }
    }
    public static void main(String[] args) {
        try {
            if (args.length == 1 && "permission-server".equals(args[0])) { permissionServer(); return; }
            if (args.length == 4 && "permission-check".equals(args[0])) {
                System.err.println("PERMISSION_ENTERED_MAIN");
                System.err.flush();
                if (System.err.checkError()) throw new IllegalStateException("permission entry stderr failed");
                System.err.println("PERMISSION_PHASE main pid=" + android.os.Process.myPid() + " uid=" + android.os.Process.myUid());
                System.err.flush();
                if (android.os.Process.myUid() != 1000 || android.os.Binder.getCallingUid() != 1000
                        || android.os.Binder.getCallingPid() != android.os.Process.myPid()) {
                    throw new IllegalStateException("original permission caller credentials differ");
                }
                System.err.println("PERMISSION_PHASE getService pid=" + android.os.Process.myPid() + " uid=" + android.os.Process.myUid());
                System.err.flush();
                var activity = android.app.ActivityManager.getService();
                if (activity == null) throw new IllegalStateException("original activity permission owner unavailable");
                System.err.println("PERMISSION_PHASE checkPermission pid=" + android.os.Process.myPid() + " uid=" + android.os.Process.myUid());
                System.err.flush();
                int status = activity.checkPermission(args[1], Integer.parseInt(args[2]), Integer.parseInt(args[3]));
                System.err.println("PERMISSION_PHASE reply pid=" + android.os.Process.myPid() + " uid=" + android.os.Process.myUid() + " status=" + status);
                System.err.flush();
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

package com.android.server.pm;

import android.content.Context;
import android.content.pm.Checksum;
import android.content.pm.IOnChecksumsReadyListener;
import android.content.pm.PackageManagerInternal;
import android.os.Binder;
import android.os.Handler;
import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelableException;
import android.os.Process;
import android.os.SELinux;
import android.os.incremental.IncrementalManager;
import android.system.ErrnoException;
import android.system.Os;
import android.util.Pair;
import android.util.ExceptionUtils;
import com.android.server.LocalServices;
import dev.aim.server.IPackageInstallerFiles;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.IOException;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.Executor;

/** Typed original helper algorithms; no PackageManagerService object or call. */
public final class PackageInstallerFileBridge extends IPackageInstallerFiles.Stub {
    private final Context context;
    private final Handler handler;
    private final Executor executor;

    public PackageInstallerFileBridge(Context context, Handler handler, Executor executor) {
        this.context = java.util.Objects.requireNonNull(context);
        this.handler = java.util.Objects.requireNonNull(handler);
        this.executor = java.util.Objects.requireNonNull(executor);
    }
    public static PackageInstallerFileBridge create() {
        android.app.ActivityThread thread = android.app.ActivityThread.currentActivityThread();
        if (thread == null) throw new IllegalStateException("system context unavailable");
        return new PackageInstallerFileBridge(thread.getSystemContext(),
            com.android.internal.os.BackgroundThread.getHandler(), com.android.internal.os.BackgroundThread.getExecutor());
    }
    private static void enforceNativeCaller() {
        if (Binder.getCallingUid() != Process.SYSTEM_UID)
            throw new SecurityException("native installer file owner requires system UID");
    }
    private static Parcel decode(byte[] bytes) {
        Parcel p = Parcel.obtain(); p.unmarshall(java.util.Objects.requireNonNull(bytes), 0, bytes.length);
        p.setDataPosition(0); return p;
    }
    @Override public byte[] verifyChecksums(byte[] bytes, byte[] signature) {
        enforceNativeCaller();
        Parcel p = decode(bytes);
        try {
            int count = p.readInt();
            if (count < 0 || count > bytes.length / 4) throw new IllegalArgumentException("invalid checksum count");
            Checksum[] checksums = new Checksum[count];
            for (int i = 0; i < count; i++) checksums[i] = p.readInt() == 0 ? null : Checksum.CREATOR.createFromParcel(p);
            p.enforceNoDataAvail();
            Certificate[] verified;
            try { verified = ApkChecksums.verifySignature(checksums, signature); }
            catch (IOException | java.security.NoSuchAlgorithmException | java.security.SignatureException e) {
                throw new IllegalArgumentException("Can't verify signature: " + e.getMessage(), e);
            }
            Parcel out = Parcel.obtain();
            try {
                out.writeInt(verified.length);
                for (Certificate certificate : verified) out.writeByteArray(certificate.getEncoded());
                return out.marshall();
            } catch (java.security.cert.CertificateEncodingException e) {
                throw ExceptionUtils.propagate(e);
            } finally { out.recycle(); }
        } finally { p.recycle(); }
    }
    private static Certificate[] certificates(byte[] bytes) {
        if (bytes == null) return null;
        Parcel p = decode(bytes);
        try {
            ArrayList<?> values = p.readArrayList(PackageInstallerFileBridge.class.getClassLoader());
            if (values == null) throw new IllegalArgumentException("null certificate list record");
            CertificateFactory factory = CertificateFactory.getInstance("X.509");
            Certificate[] result = new Certificate[values.size()];
            for (int i = 0; i < values.size(); i++) result[i] = factory.generateCertificate(new ByteArrayInputStream((byte[]) values.get(i)));
            p.enforceNoDataAvail(); return result;
        } catch (java.security.cert.CertificateException e) { throw ExceptionUtils.propagate(e); }
        finally { p.recycle(); }
    }
    @Override public void requestChecksums(String guestPath, String installerPackage, int optional,
            int required, byte[] trusted, IBinder callback) {
        enforceNativeCaller();
        File file = new File(java.util.Objects.requireNonNull(guestPath));
        if (!file.exists()) throw new ParcelableException(new FileNotFoundException(file.getAbsolutePath()));
        List<Pair<String, File>> files = new ArrayList<>(1); files.add(new Pair<String, File>(null, file));
        dispatch(files, installerPackage, optional, required, trusted, callback);
    }
    @Override public void requestPackageChecksums(byte[] bytes, String installerPackage, int optional,
            int required, byte[] trusted, IBinder callback) {
        enforceNativeCaller();
        Parcel p = decode(bytes);
        List<Pair<String, File>> files = new ArrayList<>();
        try {
            int count = p.readInt();
            if (count < 1 || count > bytes.length / 8) throw new IllegalArgumentException("invalid checksum file list");
            for (int i = 0; i < count; i++) files.add(new Pair<String, File>(p.readString(), new File(java.util.Objects.requireNonNull(p.readString()))));
            p.enforceNoDataAvail();
        } finally { p.recycle(); }
        dispatch(files, installerPackage, optional, required, trusted, callback);
    }
    private void dispatch(List<Pair<String, File>> files, String installerPackage, int optional,
            int required, byte[] trusted, IBinder callback) {
        Certificate[] certificates = certificates(trusted);
        IOnChecksumsReadyListener listener = IOnChecksumsReadyListener.Stub.asInterface(java.util.Objects.requireNonNull(callback));
        PackageManagerInternal packages = LocalServices.getService(PackageManagerInternal.class);
        if (packages == null) throw new IllegalStateException("native package internal owner unavailable");
        executor.execute(() -> {
            ApkChecksums.Injector injector = new ApkChecksums.Injector(() -> context, () -> handler,
                () -> java.util.Objects.requireNonNull(context.getSystemService(IncrementalManager.class), "incremental file owner unavailable"),
                () -> {
                    if (LocalServices.getService(PackageManagerInternal.class) != packages)
                        throw new IllegalStateException("package checksum owner replaced");
                    return packages;
                });
            ApkChecksums.getChecksums(files, optional, required, installerPackage, certificates,
                new IOnChecksumsReadyListener.Stub() {
                    @Override public void onChecksumsReady(List<android.content.pm.ApkChecksum> values) throws android.os.RemoteException {
                        if (LocalServices.getService(PackageManagerInternal.class) != packages)
                            throw new IllegalStateException("package checksum owner replaced");
                        listener.onChecksumsReady(values);
                    }
                }, injector);
        });
    }
    @Override public void stageViaHardLink(String source, String stageDirectory) {
        enforceNativeCaller();
        File target = new File(java.util.Objects.requireNonNull(source));
        File stage = new File(java.util.Objects.requireNonNull(stageDirectory));
        File destination = new File(stage, target.getName());
        try {
            try { Os.link(source, destination.getAbsolutePath()); Os.chmod(destination.getAbsolutePath(), 0644); }
            catch (ErrnoException e) { throw e.rethrowAsIOException(); }
            if (!SELinux.restorecon(destination)) throw new IOException("Can't relabel file: " + destination);
        } catch (IOException failure) {
            try { Os.unlink(destination.getAbsolutePath()); }
            catch (ErrnoException cleanup) { android.util.Slog.d("PackageInstallerFileBridge", "Failed to unlink session file: " + destination, cleanup); }
            throw new ParcelableException(failure);
        }
    }
}

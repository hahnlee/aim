package com.android.server.pm;

import android.content.Context;
import android.content.pm.IOnChecksumsReadyListener;
import android.content.pm.PackageManager;
import android.content.pm.PackageManagerInternal;
import android.os.Binder;
import android.os.Handler;
import android.os.ParcelableException;
import android.os.incremental.IncrementalManager;
import android.util.Pair;
import com.android.server.LocalServices;
import dev.aim.server.PackageSnapshots;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.util.ArrayList;
import java.util.List;
import java.util.Objects;
import java.util.concurrent.Executor;

/** Original ApkChecksums computation with native captured package/installer inputs. */
public final class NativePackageChecksums {
    private final Context context;
    private final PackageSnapshots.Store packages;
    public NativePackageChecksums(Context context, PackageSnapshots.Store packages) {
        this.context = Objects.requireNonNull(context);
        this.packages = Objects.requireNonNull(packages);
    }
    @SuppressWarnings("rawtypes")
    public void request(String name, boolean splits, int optional, int required, List trusted,
            IOnChecksumsReadyListener listener, int user, Executor executor, Handler handler) {
        Objects.requireNonNull(name); Objects.requireNonNull(listener);
        Objects.requireNonNull(executor); Objects.requireNonNull(handler);
        var files = new ArrayList<Pair<String, File>>();
        String installer;
        try (var snapshot = packages.computer()) {
            var app = snapshot.getApplicationInfoInternal(name, 0, Binder.getCallingUid(), user);
            if (app == null) throw new ParcelableException(new PackageManager.NameNotFoundException(name));
            android.content.pm.InstallSourceInfo source;
            try { source = snapshot.readQueries().getInstallSourceInfo(name, user); }
            catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            if (source == null) installer = null;
            else {
                String initiating = source.getInitiatingPackageName();
                installer = initiating == null || initiating.equals("com.android.shell")
                        ? source.getInstallingPackageName() : initiating;
            }
            files.add(new Pair<>(null, new File(app.sourceDir)));
            if (splits && app.splitNames != null) for (int i = 0; i < app.splitNames.length; i++)
                files.add(new Pair<>(app.splitNames[i], new File(app.splitSourceDirs[i])));
        }
        Certificate[] certificates = null;
        if (trusted != null) try {
            var factory = CertificateFactory.getInstance("X.509");
            certificates = new Certificate[trusted.size()];
            for (int i = 0; i < certificates.length; i++)
                certificates[i] = factory.generateCertificate(new ByteArrayInputStream((byte[]) trusted.get(i)));
        } catch (java.security.cert.CertificateException failure) {
            throw android.util.ExceptionUtils.propagate(failure);
        }
        Certificate[] captured = certificates;
        PackageManagerInternal owner = Objects.requireNonNull(LocalServices.getService(PackageManagerInternal.class),
                "native package internal owner unavailable");
        executor.execute(() -> {
            var injector = new ApkChecksums.Injector(() -> context, () -> handler,
                    () -> context.getSystemService(IncrementalManager.class), () -> {
                        if (LocalServices.getService(PackageManagerInternal.class) != owner)
                            throw new IllegalStateException("package checksum owner replaced");
                        return owner;
                    });
            ApkChecksums.getChecksums(files, optional, required, installer, captured, listener, injector);
        });
    }
}

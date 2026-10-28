package com.android.server.art;

import android.os.ArtModuleServiceManager;
import android.os.UserHandle;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.pkg.AndroidPackage;
import com.android.server.pm.pkg.AndroidPackageSplit;
import com.android.server.pm.pkg.PackageState;
import java.lang.reflect.Proxy;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;

/** Runs the image's original owner, protobuf reader/writer and ART JNI without SystemServer. */
public final class OriginalDexUseStartup {
    private static final String NAME = "a.b";
    private static final String APK = "/data/app/a.b/base.apk";

    @SuppressWarnings("unchecked")
    private static <T> T proxy(Class<T> type, java.lang.reflect.InvocationHandler handler) {
        return (T) Proxy.newProxyInstance(type.getClassLoader(), new Class<?>[] {type}, handler);
    }

    public static void main(String[] args) throws Exception {
        // Loading persisted state must not consult PackageManagerLocal or ArtManagerLocal.
        PackageManagerLocal local = proxy(PackageManagerLocal.class, (p, method, values) -> {
            throw new AssertionError("startup consulted " + method);
        });
        ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1) {
            @Override public ScheduledFuture<?> schedule(Runnable command, long delay, TimeUnit unit) {
                // Only the clock boundary changes: run the original debounce/save without 15s wait.
                return super.schedule(command, 0, TimeUnit.MILLISECONDS);
            }
        };
        try {
            if (!args[0].equals("uninitialized")) {
                ArtModuleServiceInitializer.setArtModuleServiceManager(new ArtModuleServiceManager());
            }
            DexUseManagerLocal owner = new DexUseManagerLocal(new DexUseManagerLocal.Injector(null) {
                @Override public PackageManagerLocal getPackageManagerLocal() { return local; }
                @Override public String getFilename() { return "/data/system/package-dex-usage.pb"; }
                @Override public int getCallingUid() { return 10100; }
                @Override public UserHandle getCallingUserHandle() { return UserHandle.of(0); }
                @Override public java.util.concurrent.ScheduledExecutorService createScheduledExecutor() {
                    return executor;
                }
            });
            long before = owner.getPackageLastUsedAtMs(NAME);
            if (args[0].equals("empty")) {
                if (before != 0) throw new AssertionError("empty owner contains records");
            } else if (args[0].equals("existing")) {
                if (before != 123) throw new AssertionError("existing record lost: " + before);
            } else if (args[0].equals("record")) {
                AndroidPackageSplit split = proxy(AndroidPackageSplit.class, (p, method, values) -> {
                    if (method.getName().equals("getPath")) return APK;
                    throw new AssertionError(method);
                });
                AndroidPackage pkg = proxy(AndroidPackage.class, (p, method, values) -> {
                    if (method.getName().equals("getSplits")) return List.of(split);
                    throw new AssertionError(method);
                });
                PackageState state = proxy(PackageState.class, (p, method, values) -> {
                    if (method.getName().equals("getAndroidPackage")) return pkg;
                    if (method.getName().equals("getPackageName")) return NAME;
                    throw new AssertionError(method);
                });
                try (var snapshot = proxy(PackageManagerLocal.FilteredSnapshot.class, (p, method, values) -> {
                    if (method.getName().equals("getPackageState") && NAME.equals(values[0])) return state;
                    if (method.getName().equals("close")) return null;
                    throw new AssertionError(method);
                })) {
                    owner.notifyDexContainersLoaded(snapshot, NAME, Map.of(APK, "PCL[]"));
                }
                if (!executor.awaitTermination(10, TimeUnit.SECONDS)) throw new AssertionError("save did not finish");
                if (owner.getPackageLastUsedAtMs(NAME) <= 123) throw new AssertionError("load was not recorded");
            } else if (args[0].equals("reload")) {
                if (before <= 123) throw new AssertionError("saved load was not restored: " + before);
            } else {
                throw new AssertionError("uninitialized owner started");
            }
            System.out.println("original dex-use owner: " + args[0] + " passed");
        } finally {
            executor.shutdownNow();
        }
    }
}

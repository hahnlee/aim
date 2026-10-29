/*
 * Copyright (C) 2006 The Android Open Source Project
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *     http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package com.android.server.art;

import android.content.Context;
import android.os.Binder;
import android.os.UserHandle;
import android.util.ArrayMap;
import com.android.server.pm.InstructionSets;
import dev.aim.server.IDexUseBridge;
import dev.aim.server.IDexUsePackages;
import dev.aim.server.PackageLocal;
import dev.aim.server.PackageSnapshots;
import dalvik.system.VMRuntime;
import java.io.IOException;
import java.util.Objects;

/**
 * android-16.0.0_r1 PackageManagerService.notifyDexLoad's ART branch, over native
 * package captures. Source: frameworks/base/services/core/java/com/android/server/pm/
 * PackageManagerService.java:5611-5683, tag android-16.0.0_r1 (Apache-2.0).
 * Local changes: typed native snapshots and explicit authenticated Binder identity.
 * The image's DexUseManagerLocal owns validation, discovery,
 * recording, debounce, atomic protobuf persistence and reload, unchanged.
 * Only its Binder identity and PackageManagerLocal dependencies cross the bridge.
 * No ArtManagerLocal/systemReady is needed for this write path; dexopt and the
 * shutdown/background cleanup lifecycle remain with the ART service (#2063).
 */
@SuppressWarnings("try")
public final class AppHostDexUse extends IDexUseBridge.Stub {
    private final PackageSnapshots.Store packages;
    private final PackageLocal local;
    private final ThreadLocal<Integer> caller = new ThreadLocal<>();
    private final DexUseManagerLocal usage;

    public AppHostDexUse(Context context, IDexUsePackages backend) {
        Objects.requireNonNull(backend);
        if (com.android.server.LocalManagerRegistry.getManager(DexUseManagerLocal.class) != null)
            throw new IllegalStateException("ART dex-use owner already registered");
        packages = new PackageSnapshots.Store(backend::getSnapshot, new PackageSnapshots.Owner() {
            @Override public String getFilteredPackageName(long version, String name, int uid, int user) {
                try { return backend.filterPackageName(version, name, uid, user); }
                catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
            @Override public boolean shouldFilter(long version, com.android.server.pm.pkg.PackageState state, int uid, int user) {
                try { return backend.shouldFilter(version, state.getPackageName(), uid, user); }
                catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
            }
        }, crossUser(backend));
        refresh();
        local = new PackageLocal(packages,
                (volume, name, dirs, user, app, previous, seInfo, flags) -> {
                    throw new UnsupportedOperationException("dex-use snapshot cannot reconcile SDK data");
                }, new PackageLocal.SigningOwner() {
                    @Override public void add(android.content.pm.SigningDetails oldDetails, android.content.pm.SigningDetails newDetails) { throw new UnsupportedOperationException("dex-use snapshot cannot override signing"); }
                    @Override public void remove(android.content.pm.SigningDetails details) { throw new UnsupportedOperationException("dex-use snapshot cannot override signing"); }
                    @Override public void clear() { throw new UnsupportedOperationException("dex-use snapshot cannot override signing"); }
                });
        usage = new DexUseManagerLocal(new DexUseManagerLocal.Injector(context) {
            @Override public com.android.server.pm.PackageManagerLocal getPackageManagerLocal() { return local; }
            @Override public int getCallingUid() { return Objects.requireNonNull(caller.get()); }
            @Override public UserHandle getCallingUserHandle() { return UserHandle.of(UserHandle.getUserId(getCallingUid())); }
        });
        com.android.server.LocalManagerRegistry.addManager(DexUseManagerLocal.class, usage);
    }

    private static boolean crossUser(IDexUsePackages backend) {
        try { return backend.crossUserSuspensions(); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
    }

    private void refresh() {
        try { packages.refresh(); }
        catch (android.os.RemoteException failure) { throw failure.rethrowFromSystemServer(); }
        catch (IOException failure) { throw new IllegalStateException("dex-use package capture failed", failure); }
    }

    @Override public void notifyDexLoad(String name, String[] paths, String[] contexts,
            String loaderIsa, int uid) {
        if (Binder.getCallingUid() != 1000) throw new SecurityException("Native dex-use owner required");
        if ((paths == null) != (contexts == null) || (paths != null && paths.length != contexts.length))
            throw new IllegalArgumentException("Dex path/context owners differ");
        ArrayMap<String, String> map = null;
        if (paths != null) {
            map = new ArrayMap<>();
            for (int i = 0; i < paths.length; i++) map.put(paths[i], contexts[i]);
        }
        refresh();
        caller.set(uid);
        try (var snapshot = local.withFilteredSnapshot(uid, UserHandle.of(UserHandle.getUserId(uid)))) {
            var state = snapshot.getPackageState(name);
            if (loaderIsa != null && state != null) {
                String abi = state.getPrimaryCpuAbi();
                if (abi == null) abi = android.os.Build.SUPPORTED_ABIS[0];
                String expected = InstructionSets.getDexCodeInstructionSet(VMRuntime.getInstructionSet(abi));
                if (!loaderIsa.equals(expected)) {
                    android.util.Slog.wtf("PackageManager", "Invalid loaderIsa in notifyDexLoad call from "
                            + name + ", uid " + uid + ": expected " + expected + ", got " + loaderIsa);
                    return;
                }
            }
            usage.notifyDexContainersLoaded(snapshot, name, map);
        } finally { caller.remove(); }
    }
}

/*
 * Copyright (C) 2022 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package dev.aim.server;

import android.content.pm.SigningDetails;
import android.os.Binder;
import android.os.Build;
import android.os.UserHandle;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.pkg.PackageState;
import java.io.IOException;
import java.util.List;
import java.util.Objects;

/** PackageManagerLocal over the C facade's published native replica (#836). */
public final class PackageLocal implements PackageManagerLocal {
    @FunctionalInterface
    public interface SdkDataOwner {
        void reconcile(String volumeUuid, String packageName, List<String> subDirNames,
                int userId, int appId, int previousAppId, String seInfo, int flags) throws IOException;
    }

    public interface SigningOwner {
        void add(SigningDetails oldDetails, SigningDetails newDetails);
        void remove(SigningDetails oldDetails);
        void clear();
    }

    private final PackageSnapshots.Store snapshots;
    private final SdkDataOwner sdkData;
    private final SigningOwner signing;

    public PackageLocal(PackageSnapshots.Store snapshots, SdkDataOwner sdkData, SigningOwner signing) {
        this.snapshots = Objects.requireNonNull(snapshots);
        this.sdkData = Objects.requireNonNull(sdkData);
        this.signing = Objects.requireNonNull(signing);
        snapshots.getVersion(); // A facade cannot expose an uninitialized replica.
    }

    @Override
    public void reconcileSdkData(String volumeUuid, String packageName, List<String> subDirNames,
            int userId, int appId, int previousAppId, String seInfo, int flags) throws IOException {
        sdkData.reconcile(volumeUuid, packageName, subDirNames, userId, appId, previousAppId, seInfo, flags);
    }

    @Override
    public UnfilteredSnapshot withUnfilteredSnapshot() { return snapshots.unfiltered(); }

    @Override
    public FilteredSnapshot withFilteredSnapshot() {
        return withFilteredSnapshot(Binder.getCallingUid(), Binder.getCallingUserHandle());
    }

    @Override
    public FilteredSnapshot withFilteredSnapshot(int callingUid, UserHandle user) {
        return snapshots.filtered(callingUid, user, null);
    }

    /** ART's precommit scope; its original static helper requires a C redirect (#836). */
    public FilteredSnapshot withFilteredSnapshot(PackageState uncommitted) {
        return snapshots.filtered(Binder.getCallingUid(), Binder.getCallingUserHandle(), uncommitted);
    }

    // Original PackageManagerLocalImpl's test APIs, android-16.0.0_r1.
    private static void enforceDebuggable() {
        if (!Build.isDebuggable()) {
            throw new SecurityException("This test API is only available on debuggable builds");
        }
    }

    @Override
    public void addOverrideSigningDetails(SigningDetails oldSigningDetails, SigningDetails newSigningDetails) {
        enforceDebuggable();
        signing.add(oldSigningDetails, newSigningDetails);
    }

    @Override
    public void removeOverrideSigningDetails(SigningDetails oldSigningDetails) {
        enforceDebuggable();
        signing.remove(oldSigningDetails);
    }

    @Override
    public void clearOverrideSigningDetails() {
        enforceDebuggable();
        signing.clear();
    }
}

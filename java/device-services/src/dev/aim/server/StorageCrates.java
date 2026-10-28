/*
 * Copyright (C) 2017 The Android Open Source Project
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
package dev.aim.server;

import android.content.Context;
import android.content.pm.ParceledListSlice;
import android.os.Binder;
import android.os.ParcelableException;
import android.os.storage.CrateInfo;
import android.text.TextUtils;
import com.android.server.pm.Installer;
import java.io.IOException;
import java.util.ArrayList;

/** StorageStatsService's crate leaves, android-16.0.0_r1, services/usage.
 * Native package/permission owners validate the caller before this private bridge.
 * Installer, installd, CrateInfo and ParceledListSlice remain original.
 */
final class StorageCrates extends IStorageCrates.Stub {
    private final Installer installer;

    StorageCrates(Context context) {
        installer = new Installer(context);
        installer.onStart();
    }

    @Override public ParceledListSlice<CrateInfo> queryCrates(String volumeUuid,
            String[] packageNames, int userId) {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try {
            var metadata = packageNames == null ? installer.getUserCrates(volumeUuid, userId)
                    : installer.getAppCrates(volumeUuid, packageNames, userId);
            var crates = new ArrayList<CrateInfo>();
            if (metadata != null) for (var entry : metadata) {
                if (entry == null || TextUtils.isEmpty(entry.id)
                        || TextUtils.isEmpty(entry.packageName)) continue;
                var crate = CrateInfo.copyFrom(entry.uid, entry.packageName, entry.id);
                if (crate != null) crates.add(crate);
            }
            return new ParceledListSlice<>(crates);
        } catch (Installer.InstallerException e) {
            throw new ParcelableException(new IOException(e.getMessage()));
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }
}

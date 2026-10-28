/*
 * Copyright (C) 2018 The Android Open Source Project
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

// Hash method and digest stream excerpt from android-16.0.0_r1, Apache-2.0.
package com.android.server.policy.role;
import android.app.admin.DevicePolicyManagerInternal;
import android.content.ComponentName;
import android.content.pm.PackageManagerInternal;
import android.permission.flags.Flags;
import android.provider.Settings;
import android.util.ArraySet;
import com.android.internal.util.CollectionUtils;
import com.android.server.LocalServices;
import libcore.util.HexEncoding;
import java.io.*;
import java.security.*;
public final class RoleServicePlatformHelperImpl {
    private final android.content.Context mContext;
    public RoleServicePlatformHelperImpl(android.content.Context context) { mContext = context; }
    public String computePackageStateHash(int userId) {
        PackageManagerInternal packageManagerInternal = LocalServices.getService(
                PackageManagerInternal.class);
        DevicePolicyManagerInternal devicePolicyManagerInternal = LocalServices.getService(
                DevicePolicyManagerInternal.class);
        final MessageDigestOutputStream mdos = new MessageDigestOutputStream();

        DataOutputStream dataOutputStream = new DataOutputStream(new BufferedOutputStream(mdos));
        packageManagerInternal.forEachInstalledPackage(pkg -> {
            try {
                dataOutputStream.writeUTF(pkg.getPath());
                dataOutputStream.writeInt(packageManagerInternal.getApplicationEnabledState(
                        pkg.getPackageName(), userId));

                final ArraySet<String> enabledComponents =
                        packageManagerInternal.getEnabledComponents(pkg.getPackageName(), userId);
                final int enabledComponentsSize = CollectionUtils.size(enabledComponents);
                dataOutputStream.writeInt(enabledComponentsSize);
                for (int i = 0; i < enabledComponentsSize; i++) {
                    dataOutputStream.writeUTF(enabledComponents.valueAt(i));
                }

                final ArraySet<String> disabledComponents =
                        packageManagerInternal.getDisabledComponents(pkg.getPackageName(), userId);
                final int disabledComponentsSize = CollectionUtils.size(disabledComponents);
                for (int i = 0; i < disabledComponentsSize; i++) {
                    dataOutputStream.writeUTF(disabledComponents.valueAt(i));
                }
            } catch (IOException e) {
                // Never happens for MessageDigestOutputStream and DataOutputStream.
                throw new AssertionError(e);
            }
        }, userId);
        try {
            String deviceOwner= "";
            if (devicePolicyManagerInternal != null) {
                if (devicePolicyManagerInternal.getDeviceOwnerUserId() == userId) {
                    ComponentName deviceOwnerComponent =
                        devicePolicyManagerInternal.getDeviceOwnerComponent(false);
                    if (deviceOwnerComponent != null) {
                        deviceOwner = deviceOwnerComponent.getPackageName();
                    }
                }
            }
            dataOutputStream.writeUTF(deviceOwner);
            String profileOwner = "";
            if (devicePolicyManagerInternal != null) {
                ComponentName profileOwnerComponent =
                    devicePolicyManagerInternal.getProfileOwnerAsUser(userId);
                if (profileOwnerComponent != null) {
                    profileOwner = profileOwnerComponent.getPackageName();
                }
            }
            dataOutputStream.writeUTF(profileOwner);
            dataOutputStream.writeInt(Settings.Global.getInt(mContext.getContentResolver(),
                    Settings.Global.DEVICE_DEMO_MODE, 0));
            dataOutputStream.writeBoolean(Flags.walletRoleEnabled());
            dataOutputStream.flush();
        } catch (IOException e) {
            // Never happens for MessageDigestOutputStream and DataOutputStream.
            throw new AssertionError(e);
        }
        return mdos.getDigestAsString();
    }

    private static class MessageDigestOutputStream extends OutputStream {
        private final MessageDigest mMessageDigest;

        MessageDigestOutputStream() {
            try {
                mMessageDigest = MessageDigest.getInstance("SHA256");
            } catch (NoSuchAlgorithmException e) {
                /* can't happen */
                throw new RuntimeException("Failed to create MessageDigest", e);
            }
        }

        String getDigestAsString() {
            return HexEncoding.encodeToString(mMessageDigest.digest(), true /* uppercase */);
        }

        @Override
        public void write(int b) throws IOException {
            mMessageDigest.update((byte) b);
        }

        @Override
        public void write(byte[] b) throws IOException {
            mMessageDigest.update(b);
        }

        @Override
        public void write(byte[] b, int off, int len) throws IOException {
            mMessageDigest.update(b, off, len);
        }
    }
}

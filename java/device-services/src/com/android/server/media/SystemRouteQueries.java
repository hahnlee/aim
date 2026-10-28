/*
 * Copyright 2019 The Android Open Source Project
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

package com.android.server.media;

import android.Manifest;
import android.content.Context;
import android.content.PermissionChecker;
import android.content.pm.PackageManager;
import android.media.MediaRoute2Info;
import android.media.RoutingSessionInfo;
import android.os.Binder;
import android.os.Looper;
import android.os.Parcel;
import android.os.UserHandle;
import com.android.media.flags.Flags;
import dev.aim.server.Bridge;
import dev.aim.server.ISystemRouteQueries;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;

/**
 * Cold query scope retained from MediaRouter2ServiceImpl, android-16.0.0_r1:
 * https://android.googlesource.com/platform/frameworks/base/+/refs/tags/android-16.0.0_r1/services/core/java/com/android/server/media/MediaRouter2ServiceImpl.java
 * This control path is not an audio/rendering hot path.
 * The permission checks and route/session selection are original Java; provider
 * construction is UserHandler's, using the image's original system provider.
 * Local changes: receive the Binder caller explicitly from the system-UID bridge,
 * retain only query providers (no router/manager lifecycle), refresh their volume
 * on reads, and marshal the original parcelables for the native Binder endpoint.
 * Package ownership and the v1 selected-route lookup remain in MediaRouterService's
 * native owner. Other MediaRouter2/manager operations remain explicit #1663.
 */
public final class SystemRouteQueries extends ISystemRouteQueries.Stub {
    private final Context mContext;
    private final HashMap<Integer, SystemMediaRoute2Provider> providers = new HashMap<>();
    private static final String[] BLUETOOTH_PERMISSIONS_FOR_SYSTEM_ROUTING = {
        Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_SCAN
    };

    public SystemRouteQueries(Context context) { mContext = context; }

    private SystemMediaRoute2Provider provider(int uid) {
        int userId = UserHandle.getUserId(uid);
        if (Flags.enableMirroringInMediaRouter2()) {
            throw new UnsupportedOperationException("media_router: mirroring system provider (#1663)");
        }
        SystemMediaRoute2Provider provider = providers.computeIfAbsent(userId, user ->
                SystemMediaRoute2Provider.create(mContext, UserHandle.of(user), Looper.getMainLooper()));
        provider.updateVolume();
        return provider;
    }

    @Override public synchronized byte[] getSystemRoutes(int uid, int pid,
            String callerPackageName, boolean isProxyRouter) {
        Bridge.enforceSystemUid();
        boolean hasSystemRoutingPermissions;
        if (!isProxyRouter) {
            hasSystemRoutingPermissions = checkCallerHasSystemRoutingPermissions(pid, uid);
        } else {
            hasSystemRoutingPermissions =
                    checkCallerHasPrivilegedRoutingPermissions(pid, uid, callerPackageName);
        }
        final long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            SystemMediaRoute2Provider systemProvider = provider(uid);
            List<MediaRoute2Info> systemRoutes;
            if (hasSystemRoutingPermissions) {
                var providerInfo = systemProvider.getProviderInfo();
                systemRoutes = providerInfo != null
                        ? new ArrayList<>(providerInfo.getRoutes()) : new ArrayList<>();
            } else {
                systemRoutes = new ArrayList<>();
                systemRoutes.add(systemProvider.getDefaultRoute());
            }
            out.writeTypedList(systemRoutes);
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }

    @Override public synchronized byte[] getSystemSessionInfo(int uid, int pid,
            String callerPackageName, String targetPackageName, boolean setDeviceRouteSelected) {
        Bridge.enforceSystemUid();
        boolean hasSystemRoutingPermissions;
        if (targetPackageName == null) {
            hasSystemRoutingPermissions = checkCallerHasSystemRoutingPermissions(pid, uid);
        } else {
            hasSystemRoutingPermissions =
                    checkCallerHasPrivilegedRoutingPermissions(pid, uid, callerPackageName);
        }
        final long token = Binder.clearCallingIdentity();
        Parcel out = Parcel.obtain();
        try {
            SystemMediaRoute2Provider systemProvider = provider(uid);
            RoutingSessionInfo session;
            if (hasSystemRoutingPermissions) {
                if (!Flags.enableMirroringInMediaRouter2() && setDeviceRouteSelected) {
                    session = systemProvider.generateDeviceRouteSelectedSessionInfo(targetPackageName);
                } else {
                    session = systemProvider.getSessionForPackage(targetPackageName);
                }
            } else {
                session = new RoutingSessionInfo.Builder(systemProvider.getDefaultSessionInfo())
                        .setClientPackageName(targetPackageName)
                        .build();
            }
            out.writeTypedObject(session, 0);
            return out.marshall();
        } finally {
            out.recycle();
            Binder.restoreCallingIdentity(token);
        }
    }

    private boolean checkCallerHasSystemRoutingPermissions(int pid, int uid) {
        return checkCallerHasModifyAudioRoutingPermission(pid, uid)
                || checkCallerHasBluetoothPermissions(pid, uid);
    }

    private boolean checkCallerHasPrivilegedRoutingPermissions(
            int pid, int uid, String callerPackageName) {
        return checkMediaContentControlPermission(uid, pid)
                || checkMediaRoutingControlPermission(uid, pid, callerPackageName);
    }

    private boolean checkCallerHasModifyAudioRoutingPermission(int pid, int uid) {
        return mContext.checkPermission(Manifest.permission.MODIFY_AUDIO_ROUTING, pid, uid)
                == PackageManager.PERMISSION_GRANTED;
    }

    private boolean checkCallerHasBluetoothPermissions(int pid, int uid) {
        boolean hasBluetoothRoutingPermission = true;
        for (String permission : BLUETOOTH_PERMISSIONS_FOR_SYSTEM_ROUTING) {
            hasBluetoothRoutingPermission &=
                    mContext.checkPermission(permission, pid, uid)
                            == PackageManager.PERMISSION_GRANTED;
        }
        return hasBluetoothRoutingPermission;
    }

    private boolean checkMediaContentControlPermission(int callerUid, int callerPid) {
        return mContext.checkPermission(
                        Manifest.permission.MEDIA_CONTENT_CONTROL, callerPid, callerUid)
                == PackageManager.PERMISSION_GRANTED;
    }

    private boolean checkMediaRoutingControlPermission(
            int callerUid, int callerPid, String callerPackageName) {
        if (!Flags.enablePrivilegedRoutingForMediaRoutingControl()) {
            return false;
        }

        return PermissionChecker.checkPermissionForDataDelivery(
                        mContext,
                        Manifest.permission.MEDIA_ROUTING_CONTROL,
                        callerPid,
                        callerUid,
                        callerPackageName,
                        /* attributionTag */ null,
                        /* message */ "Checking permissions for registering manager in"
                                + " MediaRouter2ServiceImpl.")
                == PermissionChecker.PERMISSION_GRANTED;
    }

}

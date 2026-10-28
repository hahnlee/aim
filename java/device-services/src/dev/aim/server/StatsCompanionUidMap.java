/*
 * Copyright (C) 2017 The Android Open Source Project
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

import static android.os.Process.THREAD_PRIORITY_BACKGROUND;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.InstallSourceInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.Signature;
import android.content.pm.SigningInfo;
import android.os.Binder;
import android.os.Bundle;
import android.os.FileUtils;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.IBinder;
import android.os.IStatsd;
import android.os.ParcelFileDescriptor;
import android.os.RemoteException;
import android.os.UserHandle;
import android.os.UserManager;
import android.util.Log;
import android.util.proto.ProtoOutputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.List;

/**
 * Cold lifecycle path retained from StatsCompanionService.java, packages/modules/StatsD
 * at android-16.0.0_r1 (Apache-2.0). The UID writer, installer/signature policy and
 * receiver bodies are original Java. Local changes: extract this scope, bind each
 * receiver cohort to its statsd generation, close pipes/threads on failure, and expose
 * a system-UID-only start bridge.
 * The native companion owns the handshake, alarms and manager; system pullers remain #1765.
 */
final class StatsCompanionUidMap extends IStatsCompanionUidMap.Stub {
    private final Context context;
    private Session session;

    StatsCompanionUidMap(Context context) { this.context = context; }

    @Override public synchronized void start(IBinder statsd) throws RemoteException {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try {
            if (session != null && session.getStatsdNonblocking() != null && session.binder == statsd) return;
            Session next = new Session(IStatsd.Stub.asInterface(statsd));
            next.start();
            if (session != null) session.close();
            session = next;
        } finally {
            Binder.restoreCallingIdentity(identity);
        }
    }

    private final class Session implements IBinder.DeathRecipient {
        private static final String TAG = "StatsCompanionService";
        private static final boolean DEBUG = false;
        private static final int APPLICATION_INFO_FIELD_ID = 1;
        private static final int UID_FIELD_ID = 1;
        private static final int VERSION_FIELD_ID = 2;
        private static final int VERSION_STRING_FIELD_ID = 3;
        private static final int PACKAGE_NAME_FIELD_ID = 4;
        private static final int INSTALLER_FIELD_ID = 5;
        private static final int CERTIFICATE_HASH_FIELD_ID = 6;
        private final Object sStatsdLock = new Object();
        private IStatsd sStatsd;
        private final IBinder binder;
        private final List<BroadcastReceiver> receivers = new ArrayList<>();
        private final HandlerThread thread = new HandlerThread(TAG);

        Session(IStatsd statsd) {
            sStatsd = statsd;
            binder = statsd.asBinder();
        }

        void start() throws RemoteException {
            thread.start();
            Handler handler = new Handler(thread.getLooper());
            try {
                IntentFilter filter = new IntentFilter(Intent.ACTION_PACKAGE_REPLACED);
                filter.addAction(Intent.ACTION_PACKAGE_ADDED);
                filter.addAction(Intent.ACTION_PACKAGE_REMOVED);
                filter.addDataScheme("package");
                register(new AppUpdateReceiver(), filter, handler);
                filter = new IntentFilter(Intent.ACTION_USER_INITIALIZE);
                filter.addAction(Intent.ACTION_USER_REMOVED);
                register(new UserUpdateReceiver(), filter, handler);
                filter = new IntentFilter(Intent.ACTION_REBOOT);
                filter.addAction(Intent.ACTION_SHUTDOWN);
                register(new ShutdownEventReceiver(), filter, handler);
                binder.linkToDeath(this, 0);
                informAllUids(context);
            } catch (RemoteException | RuntimeException e) {
                close();
                throw e;
            }
        }

        private void register(BroadcastReceiver receiver, IntentFilter filter, Handler handler) {
            context.registerReceiverForAllUsers(receiver, filter, null, handler);
            receivers.add(receiver);
        }

        private IStatsd getStatsdNonblocking() {
            synchronized (sStatsdLock) { return sStatsd; }
        }

        @Override public void binderDied() { close(); }

        private void close() {
            synchronized (sStatsdLock) {
                if (sStatsd == null) return;
                sStatsd = null;
                for (BroadcastReceiver receiver : receivers) context.unregisterReceiver(receiver);
                receivers.clear();
                binder.unlinkToDeath(this, 0);
                thread.quitSafely();
            }
        }

        private String getInstallerPackageName(PackageManager pm, String name) {
            InstallSourceInfo installSourceInfo = null;
            try {
                installSourceInfo = pm.getInstallSourceInfo(name);
            } catch (PackageManager.NameNotFoundException e) {
                Log.e(TAG, "Could not get installer for package: " + name, e);
            }

            String installerPackageName = null;
            if (installSourceInfo != null) {
                installerPackageName = installSourceInfo.getInitiatingPackageName();
                if (installerPackageName == null || installerPackageName.equals("com.android.shell")) {
                    installerPackageName = installSourceInfo.getInstallingPackageName();
                }
            }

            return installerPackageName == null ? "" : installerPackageName;
        }

        private byte[] getPackageCertificateHash(final SigningInfo si) {
            if (si == null) {
                return new byte[0];
            }

            final Signature[] signatures = si.getApkContentsSigners();
            if (signatures == null || signatures.length < 1) {
                return new byte[0];
            }

            MessageDigest messageDigest = null;
            try {
                messageDigest = MessageDigest.getInstance("SHA-256");
            } catch (NoSuchAlgorithmException e) {
                Log.e(TAG, "Failed to get SHA-256 instance of MessageDigest", e);
                return new byte[0];
            }

            Arrays.sort(signatures, Comparator.comparing(Signature::hashCode));
            for (final Signature signature : signatures) {
                messageDigest.update(signature.toByteArray());
            }

            return messageDigest.digest();
        }

        private void informAllUids(Context context) {
            ParcelFileDescriptor[] fds;
            try {
                fds = ParcelFileDescriptor.createPipe();
            } catch (IOException e) {
                Log.e(TAG, "Failed to create a pipe to send uid map data.", e);
                return;
            }
            HandlerThread backgroundThread = new HandlerThread(
                    "statsCompanionService.bg", THREAD_PRIORITY_BACKGROUND);
            backgroundThread.start();
            Handler handler = new Handler(backgroundThread.getLooper());
            handler.post(() -> {
                if (DEBUG) Log.d(TAG, "Start thread for sending uid map data.");
                FileOutputStream fout = new ParcelFileDescriptor.AutoCloseOutputStream(fds[1]);
                try {
                    UserManager um = (UserManager) context.getSystemService(Context.USER_SERVICE);
                    PackageManager pm = context.getPackageManager();
                    final List<UserHandle> users = um.getUserHandles(true);
                    IStatsd statsd = getStatsdNonblocking();
                    if (statsd == null) return;
                    ProtoOutputStream output = new ProtoOutputStream(fout);
                    int numRecords = 0;

                    // Add in all the apps for every user/profile.
                    for (UserHandle userHandle : users) {
                        List<PackageInfo> packagesPlusApex = getAllPackagesWithApex(pm, userHandle);
                        for (int j = 0; j < packagesPlusApex.size(); j++) {
                            if (packagesPlusApex.get(j).applicationInfo != null) {
                                final String installer = getInstallerPackageName(
                                        pm, packagesPlusApex.get(j).packageName);

                                long applicationInfoToken =
                                        output.start(ProtoOutputStream.FIELD_TYPE_MESSAGE
                                                | ProtoOutputStream.FIELD_COUNT_REPEATED
                                                | APPLICATION_INFO_FIELD_ID);
                                output.write(ProtoOutputStream.FIELD_TYPE_INT32
                                                | ProtoOutputStream.FIELD_COUNT_SINGLE | UID_FIELD_ID,
                                        packagesPlusApex.get(j).applicationInfo.uid);
                                output.write(ProtoOutputStream.FIELD_TYPE_INT64
                                                | ProtoOutputStream.FIELD_COUNT_SINGLE
                                                | VERSION_FIELD_ID,
                                        packagesPlusApex.get(j).getLongVersionCode());
                                output.write(ProtoOutputStream.FIELD_TYPE_STRING
                                                | ProtoOutputStream.FIELD_COUNT_SINGLE
                                                | VERSION_STRING_FIELD_ID,
                                        packagesPlusApex.get(j).versionName);
                                output.write(ProtoOutputStream.FIELD_TYPE_STRING
                                        | ProtoOutputStream.FIELD_COUNT_SINGLE
                                        | PACKAGE_NAME_FIELD_ID, packagesPlusApex.get(j).packageName);
                                output.write(ProtoOutputStream.FIELD_TYPE_STRING
                                                | ProtoOutputStream.FIELD_COUNT_SINGLE
                                                | INSTALLER_FIELD_ID,
                                        installer);
                                final byte[] certHash =
                                    getPackageCertificateHash(packagesPlusApex.get(j).signingInfo);
                                output.write(ProtoOutputStream.FIELD_TYPE_BYTES
                                        | ProtoOutputStream.FIELD_COUNT_SINGLE
                                        | CERTIFICATE_HASH_FIELD_ID,
                                    certHash);

                                numRecords++;
                                output.end(applicationInfoToken);
                            }
                        }
                    }
                    try {
                        // inform statsd about data is ready to be consumed to avoid blocking in
                        // statsd while reading & in this thread while writing (see flush below)
                        statsd.informAllUidData(fds[0]);
                        // close read fd since it is duped by binder transaction
                        fds[0].close();
                        output.flush();
                    } catch (RemoteException e) {
                        Log.e(TAG, "Failed to send uid map to statsd");
                    } catch (IOException e) {
                        Log.e(TAG, "Failed to close the read side of the pipe.", e);
                    } catch (RuntimeException e) {
                        if (e.getCause() != null && e.getCause() instanceof IOException) {
                            Log.e(TAG, "Failed to flush write side of the pipe.", e);
                        } else {
                            throw e;
                        }
                    }
                    if (DEBUG) {
                        Log.d(TAG, "Sent data for " + numRecords + " apps");
                    }
                } finally {
                    if (DEBUG) Log.d(TAG, "End thread for sending uid map data.");
                    FileUtils.closeQuietly(fds[0]);
                    FileUtils.closeQuietly(fout);
                    backgroundThread.quit();
                }
            });
        }

        private List<PackageInfo> getAllPackagesWithApex(PackageManager pm,
                UserHandle userHandle) {
            // We want all the uninstalled packages because uninstalled package uids can still be logged
            // to statsd.
            List<PackageInfo> allPackages = new ArrayList<>(
                    pm.getInstalledPackagesAsUser(PackageManager.GET_SIGNING_CERTIFICATES
                                    | PackageManager.MATCH_UNINSTALLED_PACKAGES
                                    | PackageManager.MATCH_ANY_USER
                                    | PackageManager.MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,
                            userHandle.getIdentifier()));
            // We make a second query to package manager for the apex modules because package manager
            // returns both installed and uninstalled apexes with
            // PackageManager.MATCH_UNINSTALLED_PACKAGES flag. We only want active apexes because
            // inactive apexes can conflict with active ones.
            for (PackageInfo packageInfo : pm.getInstalledPackages(PackageManager.MATCH_APEX)) {
                if (packageInfo.isApex) {
                    allPackages.add(packageInfo);
                }
            }
            return allPackages;
        }

        private final class AppUpdateReceiver extends BroadcastReceiver {
            @Override
            public void onReceive(Context context, Intent intent) {
                /**
                 * App updates actually consist of REMOVE, ADD, and then REPLACE broadcasts. To avoid
                 * waste, we ignore the REMOVE and ADD broadcasts that contain the replacing flag.
                 * If we can't find the value for EXTRA_REPLACING, we default to false.
                 */
                if (intent.getAction() == null) {
                    return;
                }
                if (!intent.getAction().equals(Intent.ACTION_PACKAGE_REPLACED)
                        && intent.getBooleanExtra(Intent.EXTRA_REPLACING, false)) {
                    return; // Keep only replacing or normal add and remove.
                }
                if (DEBUG) Log.d(TAG, "StatsCompanionService noticed an app was updated.");
                synchronized (sStatsdLock) {
                    if (sStatsd == null) {
                        Log.w(TAG, "Could not access statsd to inform it of an app update");
                        return;
                    }
                    try {
                        if (intent.getAction().equals(Intent.ACTION_PACKAGE_REMOVED)) {
                            Bundle b = intent.getExtras();
                            int uid = b.getInt(Intent.EXTRA_UID);
                            boolean replacing = intent.getBooleanExtra(Intent.EXTRA_REPLACING, false);
                            if (!replacing) {
                                // Don't bother sending an update if we're right about to get another
                                // intent for the new version that's added.
                                String app = intent.getData().getSchemeSpecificPart();
                                sStatsd.informOnePackageRemoved(app, uid);
                            }
                        } else {
                            PackageManager pm = context.getPackageManager();
                            Bundle b = intent.getExtras();
                            int uid = b.getInt(Intent.EXTRA_UID);
                            String app = intent.getData().getSchemeSpecificPart();
                            PackageInfo pi = pm.getPackageInfo(app,
                                        PackageManager.GET_SIGNING_CERTIFICATES
                                        | PackageManager.MATCH_ANY_USER);
                            final String installer = getInstallerPackageName(pm, app);

                            // Get Package certificate hash.
                            byte[] certHash = getPackageCertificateHash(pi.signingInfo);

                            sStatsd.informOnePackage(
                                    app,
                                    uid,
                                    pi.getLongVersionCode(),
                                    pi.versionName == null ? "" : pi.versionName,
                                    installer,
                                    certHash);
                        }
                    } catch (Exception e) {
                        Log.w(TAG, "Failed to inform statsd of an app update", e);
                    }
                }
            }
        }

        private final class UserUpdateReceiver extends BroadcastReceiver {
            @Override
            public void onReceive(Context context, Intent intent) {
                // Pull the latest state of UID->app name, version mapping.
                // Needed since the new user basically has a version of every app.
                informAllUids(context);
            }
        }

        private final class ShutdownEventReceiver extends BroadcastReceiver {
            @Override
            public void onReceive(Context context, Intent intent) {
                /**
                 * Skip immediately if intent is not relevant to device shutdown.
                 */
                if (intent.getAction() == null) {
                    return;
                }
                if (!intent.getAction().equals(Intent.ACTION_REBOOT)
                        && !(intent.getAction().equals(Intent.ACTION_SHUTDOWN)
                        && (intent.getFlags() & Intent.FLAG_RECEIVER_FOREGROUND) != 0)) {
                    return;
                }

                if (DEBUG) {
                    Log.i(TAG, "StatsCompanionService noticed a shutdown.");
                }
                IStatsd statsd = getStatsdNonblocking();
                if (statsd == null) {
                    Log.w(TAG, "Could not access statsd to inform it of a shutdown event.");
                    return;
                }
                try {
                    // two way binder call
                    statsd.informDeviceShutdown();
                } catch (Exception e) {
                    Log.w(TAG, "Failed to inform statsd of a shutdown event.", e);
                }
            }
        }
    }
}

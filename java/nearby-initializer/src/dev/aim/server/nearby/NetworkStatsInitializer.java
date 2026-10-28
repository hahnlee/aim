/*
 * Copyright (C) 2011 The Android Open Source Project
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

package dev.aim.server.nearby;

import static android.net.NetworkStats.*;
import static android.net.NetworkStatsHistory.FIELD_ALL;
import static android.net.NetworkTemplate.MATCH_TEST;
import static android.Manifest.permission.UPDATE_DEVICE_STATS;

import android.content.Context;
import android.net.INetworkStatsSession;
import android.net.NetworkStats;
import android.net.NetworkStatsCollection;
import android.net.NetworkStatsHistory;
import android.net.NetworkTemplate;
import android.net.connectivity.android.net.NetworkStatsAccess;
import android.net.connectivity.com.android.net.module.util.LocationPermissionChecker;
import android.net.connectivity.com.android.net.module.util.PermissionUtils;
import android.os.Binder;
import android.os.IBinder;
import android.telephony.SubscriptionPlan;

import com.android.server.SystemService;
import dev.aim.server.INetworkStatsBridge;
import java.util.Objects;

/**
 * Cold netstats calls in aim-system (#1671). Session/query and operation code adapted
 * from platform/packages/modules/Connectivity, android-16.0.0_r1 (Apache-2.0),
 * service-t/src/com/android/server/net/NetworkStatsService.java.
 * NetworkStatsCollection, NetworkStatsAccess, NetworkStatsHistory, NetworkStats,
 * PermissionUtils and LocationPermissionChecker remain the image's original Java.
 *
 * The host records lifetime UID socket counters, but no interface identity or
 * recorder snapshots. As in the original before any mapped snapshot, its three
 * history collections are empty; UID lifetime totals are not historical buckets.
 * Operations remain in the original NetworkStats accumulator (including TAG_NONE),
 * distinct from the collections: a recorder cannot attribute them without an iface.
 */
public final class NetworkStatsInitializer extends SystemService {
    public NetworkStatsInitializer(Context context) { super(context); }

    @Override public void onStart() {
        publishBinderService("aim.netstats", new Owner(getContext()), false);
    }

    static final class Owner extends INetworkStatsBridge.Stub {
        private static final String TAG = "NetworkStats";
        private final Context mContext;
        private final LocationPermissionChecker mLocationPermissionChecker;
        private final Object mStatsLock = new Object();
        private final NetworkStats mUidOperations = new NetworkStats(0L, 10);
        private final NetworkStatsCollection mXtStatsCached = new NetworkStatsCollection(3600000L);
        private final NetworkStatsCollection mUidStats = new NetworkStatsCollection(7200000L);
        private final NetworkStatsCollection mUidTagStats = new NetworkStatsCollection(7200000L);

        Owner(Context context) {
            mContext = context;
            mLocationPermissionChecker = new LocationPermissionChecker(context);
        }

        @Override public IBinder openSessionForUsageStats(int flags, String callingPackage) {
            Objects.requireNonNull(callingPackage);
            PermissionUtils.enforcePackageNameMatchesUid(
                    mContext, Binder.getCallingUid(), callingPackage);
            // Polling has no mapped recorder snapshots here. Neither POLL_ON_OPEN nor
            // POLL_FORCE can manufacture history from unassociated lifetime counters.
            return openSessionInternal(flags, callingPackage).asBinder();
        }

        private INetworkStatsSession openSessionInternal(final int flags,
                final String callingPackage) {
            return new INetworkStatsSession.Stub() {
                private final int mCallingUid = Binder.getCallingUid();
                private final String mCallingPackage = callingPackage;
                private final @NetworkStatsAccess.Level int mAccessLevel = NetworkStatsAccess.checkAccessLevel(
                        mContext, Binder.getCallingPid(), Binder.getCallingUid(), callingPackage);

                private NetworkStatsCollection mUidComplete;
                private NetworkStatsCollection mUidTagComplete;

                private NetworkStatsCollection getUidComplete() {
                    synchronized (mStatsLock) {
                        if (mUidComplete == null) {
                            mUidComplete = mUidStats;
                        }
                        return mUidComplete;
                    }
                }

                private NetworkStatsCollection getUidTagComplete() {
                    synchronized (mStatsLock) {
                        if (mUidTagComplete == null) {
                            mUidTagComplete = mUidTagStats;
                        }
                        return mUidTagComplete;
                    }
                }

                @Override
                public int[] getRelevantUids() {
                    return getUidComplete().getRelevantUids(mAccessLevel);
                }

                @Override
                public NetworkStats getDeviceSummaryForNetwork(
                        NetworkTemplate template, long start, long end) {
                    enforceTemplatePermissions(template, callingPackage);
                    return internalGetSummaryForNetwork(template, flags, start, end,
                            mAccessLevel, mCallingUid);
                }

                @Override
                public NetworkStats getSummaryForNetwork(
                        NetworkTemplate template, long start, long end) {
                    enforceTemplatePermissions(template, callingPackage);
                    return internalGetSummaryForNetwork(template, flags, start, end,
                            mAccessLevel, mCallingUid);
                }

                @Override
                public NetworkStatsHistory getHistoryForNetwork(NetworkTemplate template, int fields) {
                    enforceTemplatePermissions(template, callingPackage);
                    return internalGetHistoryForNetwork(template, flags, fields,
                            mAccessLevel, mCallingUid, Long.MIN_VALUE, Long.MAX_VALUE);
                }

                @Override
                public NetworkStatsHistory getHistoryIntervalForNetwork(NetworkTemplate template,
                        int fields, long start, long end) {
                    enforceTemplatePermissions(template, callingPackage);
                    return internalGetHistoryForNetwork(template, flags, fields,
                            mAccessLevel, mCallingUid, start, end);
                }

                @Override
                public NetworkStats getSummaryForAllUid(
                        NetworkTemplate template, long start, long end, boolean includeTags) {
                    enforceTemplatePermissions(template, callingPackage);
                    try {
                        final NetworkStats stats = getUidComplete()
                                .getSummary(template, start, end, mAccessLevel, mCallingUid);
                        if (includeTags) {
                            final NetworkStats tagStats = getUidTagComplete()
                                    .getSummary(template, start, end, mAccessLevel, mCallingUid);
                            stats.combineAllValues(tagStats);
                        }
                        return stats;
                    } catch (NullPointerException e) {
                        throw e;
                    }
                }

                @Override
                public NetworkStats getTaggedSummaryForAllUid(
                        NetworkTemplate template, long start, long end) {
                    enforceTemplatePermissions(template, callingPackage);
                    try {
                        final NetworkStats tagStats = getUidTagComplete()
                                .getSummary(template, start, end, mAccessLevel, mCallingUid);
                        return tagStats;
                    } catch (NullPointerException e) {
                        throw e;
                    }
                }

                @Override
                public NetworkStatsHistory getHistoryForUid(
                        NetworkTemplate template, int uid, int set, int tag, int fields) {
                    enforceTemplatePermissions(template, callingPackage);
                    // NOTE: We don't augment UID-level statistics
                    if (tag == TAG_NONE) {
                        return getUidComplete().getHistory(template, null, uid, set, tag, fields,
                                Long.MIN_VALUE, Long.MAX_VALUE, mAccessLevel, mCallingUid);
                    } else {
                        return getUidTagComplete().getHistory(template, null, uid, set, tag, fields,
                                Long.MIN_VALUE, Long.MAX_VALUE, mAccessLevel, mCallingUid);
                    }
                }

                @Override
                public NetworkStatsHistory getHistoryIntervalForUid(
                        NetworkTemplate template, int uid, int set, int tag, int fields,
                        long start, long end) {
                    enforceTemplatePermissions(template, callingPackage);
                    // NOTE: We don't augment UID-level statistics
                    if (tag == TAG_NONE) {
                        return getUidComplete().getHistory(template, null, uid, set, tag, fields,
                                start, end, mAccessLevel, mCallingUid);
                    } else if (uid == Binder.getCallingUid()) {
                        return getUidTagComplete().getHistory(template, null, uid, set, tag, fields,
                                start, end, mAccessLevel, mCallingUid);
                    } else {
                        throw new SecurityException("Calling package " + mCallingPackage
                                + " cannot access tag information from a different uid");
                    }
                }

                @Override
                public void close() {
                    mUidComplete = null;
                    mUidTagComplete = null;
                }
            };
        }
        private void enforceTemplatePermissions(NetworkTemplate template,
                String callingPackage) {
            // For a template with wifi network keys, it is possible for a malicious
            // client to track the user locations via querying data usage. Thus, enforce
            // fine location permission check.
            // For a template with MATCH_TEST, since the wifi network key is just a placeholder
            // to identify a specific test network, it is not related to track user location.
            if (!template.getWifiNetworkKeys().isEmpty() && template.getMatchRule() != MATCH_TEST) {
                final boolean canAccessFineLocation = mLocationPermissionChecker
                        .checkCallersLocationPermission(callingPackage,
                        null /* featureId */,
                                Binder.getCallingUid(),
                                false /* coarseForTargetSdkLessThanQ */,
                                null /* message */);
                if (!canAccessFineLocation) {
                    throw new SecurityException("Access fine location is required when querying"
                            + " with wifi network keys, make sure the app has the necessary"
                            + "permissions and the location toggle is on.");
                }
            }
        }

        private NetworkStats internalGetSummaryForNetwork(NetworkTemplate template, int flags,
                long start, long end, int accessLevel, int callingUid) {
            // We've been using pure XT stats long enough that we no longer need to
            // splice DEV and XT together.
            final NetworkStatsHistory history = internalGetHistoryForNetwork(template, flags, FIELD_ALL,
                    accessLevel, callingUid, Long.MIN_VALUE, Long.MAX_VALUE);

            final long now = System.currentTimeMillis();
            final NetworkStatsHistory.Entry entry = history.getValues(start, end, now, null);

            final NetworkStats stats = new NetworkStats(end - start, 1);
            stats.insertEntry(new NetworkStats.Entry(IFACE_ALL, UID_ALL, SET_ALL, TAG_NONE,
                    METERED_ALL, ROAMING_ALL, DEFAULT_NETWORK_ALL, entry.rxBytes, entry.rxPackets,
                    entry.txBytes, entry.txPackets, entry.operations));
            return stats;
        }

        /**
         * Return network history, splicing between DEV and XT stats when
         * appropriate.
         */
        private NetworkStatsHistory internalGetHistoryForNetwork(NetworkTemplate template,
                int flags, int fields, int accessLevel, int callingUid,
                long start, long end) {
            // We've been using pure XT stats long enough that we no longer need to
            // splice DEV and XT together.
            // No subscription identities or plans are recorded by this host (#1671).
            final SubscriptionPlan augmentPlan = null;
            synchronized (mStatsLock) {
                return mXtStatsCached.getHistory(template, augmentPlan,
                        UID_ALL, SET_ALL, TAG_NONE, fields, start, end, accessLevel, callingUid);
            }
        }

        @Override
        public void incrementOperationCount(int uid, int tag, int operationCount) {
            if (Binder.getCallingUid() != uid) {
                mContext.enforceCallingOrSelfPermission(UPDATE_DEVICE_STATS, TAG);
            }

            if (operationCount < 0) {
                throw new IllegalArgumentException("operation count can only be incremented");
            }
            if (tag == TAG_NONE) {
                throw new IllegalArgumentException("operation count must have specific tag");
            }

            synchronized (mStatsLock) {
                final int set = SET_DEFAULT;
                mUidOperations.combineValues(
                        null, uid, set, tag, 0L, 0L, 0L, 0L, operationCount);
                mUidOperations.combineValues(
                        null, uid, set, TAG_NONE, 0L, 0L, 0L, 0L, operationCount);
            }
        }

    }
}

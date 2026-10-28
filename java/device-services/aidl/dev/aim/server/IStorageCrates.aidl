package dev.aim.server;
import android.content.pm.ParceledListSlice;

/** Cold StorageStatsService crate conversion; system UID only. */
interface IStorageCrates {
    /** Null packageNames selects getUserCrates; an empty array selects no apps. */
    ParceledListSlice queryCrates(String volumeUuid, in String[] packageNames, int userId);
}

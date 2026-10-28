package dev.aim.server;
import android.content.pm.ParceledListSlice;
import android.os.storage.CrateInfo;

/** Cold StorageStatsService crate conversion; system UID only. */
@JavaPassthrough(annotation="@SuppressWarnings(dev.aim.server.IStorageCrates.GENERIC_RETURN_WARNING)")
interface IStorageCrates {
    const String GENERIC_RETURN_WARNING = "unchecked";
    /** Null packageNames selects getUserCrates; an empty array selects no apps. */
    ParceledListSlice<CrateInfo> queryCrates(String volumeUuid, in String[] packageNames, int userId);
}

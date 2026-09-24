package android.app.servertransaction;

import android.os.IBinder;
import android.util.MergedConfiguration;
import android.window.ActivityWindowInfo;
import java.util.List;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public class ActivityRelaunchItem extends ClientTransactionItem {
    public ActivityRelaunchItem(IBinder activityToken, List<?> pendingResults,
            List<?> pendingNewIntents, int configChanges, MergedConfiguration config,
            boolean preserveWindow, ActivityWindowInfo activityWindowInfo) {}
}

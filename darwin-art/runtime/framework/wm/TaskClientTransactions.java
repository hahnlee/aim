package dev.darwinart.runtime.wm;

import android.app.IApplicationThread;
import android.app.servertransaction.ActivityConfigurationChangeItem;
import android.app.servertransaction.ActivityRelaunchItem;
import android.app.servertransaction.ClientTransaction;
import android.app.servertransaction.ClientTransactionItem;
import android.app.servertransaction.ConfigurationChangeItem;
import android.app.servertransaction.PauseActivityItem;
import android.app.servertransaction.ActivityLifecycleItem;
import android.app.servertransaction.ResumeActivityItem;
import android.app.servertransaction.StopActivityItem;
import android.app.servertransaction.WindowStateResizeItem;
import android.content.res.Configuration;
import android.graphics.Rect;
import android.os.IBinder;
import android.os.RemoteException;
import android.util.MergedConfiguration;
import android.view.IWindow;
import android.view.InsetsState;
import android.window.ActivityWindowInfo;
import android.window.ClientWindowFrames;
import java.util.List;

/**
 * Builds original Android 16 client transaction items for a task geometry
 * change and schedules them through ClientTransaction/IApplicationThread.
 * ActivityThread's TransactionExecutor owns every callback these items cause;
 * this class never calls a lifecycle or IWindow method directly.
 */
final class TaskClientTransactions {
    private TaskClientTransactions() {}

    static ClientTransactionItem processConfiguration(Configuration global) {
        return new ConfigurationChangeItem(new Configuration(global), 0 /* default device */);
    }

    static ClientTransactionItem activityConfiguration(IBinder token, Configuration override) {
        return new ActivityConfigurationChangeItem(
                token, new Configuration(override), new ActivityWindowInfo());
    }

    /**
     * The framework relaunch path for an Activity that does not handle
     * {@code changes}. The lifecycle item returns it to its current
     * {@link ActivityLifecycleItem} state.
     */
    static void relaunch(List<ClientTransactionItem> items, IBinder token, int changes,
            Configuration global, Configuration override, int lifecycleState) {
        items.add(new ActivityRelaunchItem(token, null, null, changes,
                new MergedConfiguration(new Configuration(global), new Configuration(override)),
                false /* preserveWindow */, new ActivityWindowInfo()));
        switch (lifecycleState) {
            case ActivityLifecycleItem.ON_RESUME:
                items.add(new ResumeActivityItem(token, false /* isForward */,
                        false /* shouldSendCompatFakeFocus */));
                break;
            case ActivityLifecycleItem.ON_STOP:
                items.add(stop(token));
                break;
            default:
                items.add(new PauseActivityItem(token));
        }
    }

    /** ActivityTaskSupervisor stop of an Activity that is no longer visible. */
    static ClientTransactionItem stop(IBinder token) {
        return new StopActivityItem(token);
    }

    static ClientTransactionItem windowResize(IBinder window, Rect frame, Rect display,
            Configuration global, Configuration override, int seq) {
        ClientWindowFrames frames = new ClientWindowFrames();
        frames.frame.set(frame);
        frames.displayFrame.set(display);
        frames.parentFrame.set(display);
        frames.compatScale = 1.0f;
        frames.seq = seq;
        return new WindowStateResizeItem(IWindow.Stub.asInterface(window), frames,
                false /* reportDraw: BLAST sync completion owns draw reporting */,
                new MergedConfiguration(new Configuration(global), new Configuration(override)),
                new InsetsState(), true /* forceLayout */, false /* alwaysConsumeSystemBars */,
                0 /* task display */, -1 /* no sync seq */, false /* dragResizing */,
                new ActivityWindowInfo());
    }

    static void schedule(IBinder applicationThread, List<ClientTransactionItem> items)
            throws RemoteException {
        if (items.isEmpty()) return;
        IApplicationThread client = IApplicationThread.Stub.asInterface(applicationThread);
        if (client == null) throw new IllegalArgumentException("application thread is required");
        ClientTransaction transaction = new ClientTransaction(client);
        for (ClientTransactionItem item : items) transaction.addTransactionItem(item);
        RemoteException failure = transaction.schedule();
        if (failure != null) throw failure;
    }
}

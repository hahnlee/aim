package android.app.servertransaction;

import android.util.MergedConfiguration;
import android.view.IWindow;
import android.view.InsetsState;
import android.window.ActivityWindowInfo;
import android.window.ClientWindowFrames;

/** Compile-only Android 16 signature stub; runtime resolution uses framework.jar. */
public class WindowStateResizeItem extends ClientTransactionItem {
    public WindowStateResizeItem(IWindow window, ClientWindowFrames frames, boolean reportDraw,
            MergedConfiguration configuration, InsetsState insetsState, boolean forceLayout,
            boolean alwaysConsumeSystemBars, int displayId, int syncSeqId, boolean dragResizing,
            ActivityWindowInfo activityWindowInfo) {}
}

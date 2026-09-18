package dev.darwinart.runtime.wm;

import android.view.WindowManager;

public final class DesktopWindowMetadataRegistry {
    public static boolean failUpdate;
    public static boolean failRemove;
    public static int updateCount;
    public static int removeCount;
    public static void reset() { failUpdate=false; failRemove=false; updateCount=0; removeCount=0; }
    public void update(int pid, android.os.IBinder window, WindowManager.LayoutParams attrs, int visibility) {
        ++updateCount;
        if (failUpdate) throw new RuntimeException("injected metadata update failure");
    }
    public void remove(int pid, android.os.IBinder window) {
        ++removeCount;
        if (failRemove) throw new RuntimeException("injected metadata remove failure");
    }
}

package android.content.pm;

import android.content.Context;
import java.lang.reflect.Field;
import java.util.List;

/** In-process shortcut service client for a detached Android app process. */
public final class ProbeShortcutManager extends ShortcutManager {
    public ProbeShortcutManager(Context context) {
        super();
        try {
            // The SDK stub exposes only ShortcutManager's legacy no-argument
            // constructor, while the Android runtime stores the Context in a
            // hidden constructor. Supply the same field during host service
            // bootstrap so inherited helpers retain normal framework state.
            Field contextField = ShortcutManager.class.getDeclaredField("mContext");
            contextField.setAccessible(true);
            contextField.set(this, context);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Could not attach ShortcutManager context", error);
        }
    }

    @Override
    public boolean isRequestPinShortcutSupported() {
        // The macOS host currently has no launcher shortcut surface. Android
        // reports that capability through IShortcutService instead of failing
        // the caller when the launcher does not support pinned shortcuts.
        return false;
    }

    @Override
    public void reportShortcutUsed(String shortcutId) {
        // system_server normally records this usage signal. The standalone
        // host has no launcher shortcut database, so accepting it is enough.
    }

    @Override
    public boolean setDynamicShortcuts(List<ShortcutInfo> shortcutInfoList) {
        return true;
    }

    @Override
    public boolean updateShortcuts(List<ShortcutInfo> shortcutInfoList) {
        return true;
    }
}

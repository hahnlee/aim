package android.view;

import android.os.IBinder;

public interface WindowManager {
    final class LayoutParams {
        public int type = 1;
        public int flags;
        public IBinder token;
    }
}

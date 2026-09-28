package android.view;

/** Test stub: the LayoutParams flags WindowInputPublisher reads (AOSP values). */
public interface WindowManager {
    class LayoutParams {
        public static final int FLAG_NOT_FOCUSABLE = 0x00000008;
        public static final int FLAG_NOT_TOUCHABLE = 0x00000010;
        public static final int FLAG_NOT_TOUCH_MODAL = 0x00000020;
        public static final int FLAG_WATCH_OUTSIDE_TOUCH = 0x00040000;
    }
}

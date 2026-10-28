import android.os.Binder;
import android.text.TextUtils;
import dev.aim.server.SafeLabels;

public final class SafeLabelsTest {
    public static void main(String[] args) {
        SafeLabels bridge = new SafeLabels();
        for (String label : new String[] {"", "Clock", "  <b>a&amp;b</b><br>c  ",
                "a\nb\u2028c", "🙂", "a".repeat(1100)}) {
            int calls = TextUtils.calls;
            if (!bridge.makeSafeLabel(label).equals("original result…")
                    || TextUtils.input != label || TextUtils.calls != calls + 1)
                throw new AssertionError("original call/result changed");
        }
        for (int uid : new int[] {0, 2000, 10100, 101000}) {
            Binder.uid = uid;
            int calls = TextUtils.calls;
            try {
                bridge.makeSafeLabel("denied");
                throw new AssertionError("non-system caller reached original");
            } catch (SecurityException expected) {}
            if (TextUtils.calls != calls) throw new AssertionError("guard ran after original");
        }
        Binder.uid = 1000;
        RuntimeException failure = new NullPointerException("original null input");
        TextUtils.failure = failure;
        try {
            bridge.makeSafeLabel(null);
            throw new AssertionError("original failure swallowed");
        } catch (RuntimeException expected) {
            if (expected != failure || TextUtils.input != null) throw new AssertionError(expected);
        }
    }
}

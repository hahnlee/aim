// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Intent;
import android.os.Bundle;
import android.view.ContextThemeWrapper;

public class Activity extends ContextThemeWrapper {
    public static final int RESULT_OK = -1;

    public Activity() { throw new RuntimeException("stub"); }
    protected void onCreate(Bundle savedInstanceState) { throw new RuntimeException("stub"); }
    public Intent getIntent() { throw new RuntimeException("stub"); }
    public final void setResult(int resultCode, Intent data) { throw new RuntimeException("stub"); }
    public void finish() { throw new RuntimeException("stub"); }
    public final void runOnUiThread(Runnable action) { throw new RuntimeException("stub"); }
}

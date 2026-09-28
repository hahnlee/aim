package dev.aim.probe;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;

/** Isolated Button acceptance Activity loaded in place of the baseline ProbeActivity. */
public final class ProbeActivity extends Activity {
    private int lifecycleValue;

    @Override
    protected void attachBaseContext(Context newBase) {
        // Preserve the normal Android ContextWrapper base so PhoneWindow and
        // LayoutInflater see the APK PathClassLoader/resources from the
        // attached ProbeContext.
        super.attachBaseContext(newBase);
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        FontBootstrap.install();
        setContentView(new ProbeView(this));
        lifecycleValue = 43;
    }

    public int probeOnCreate() {
        onCreate(null);
        return lifecycleValue;
    }

    public int probeValue() {
        return 42;
    }
}

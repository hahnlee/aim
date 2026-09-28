package dev.aim.probe;

import android.app.Activity;
import android.content.Context;
import android.os.Bundle;

public final class ProbeActivity extends Activity {
    private int lifecycleValue;

    @Override
    protected void attachBaseContext(Context newBase) {
        // The native launcher attaches ContextThemeWrapper non-virtually.
        // Android's Activity override additionally installs autofill/content
        // capture clients, which are outside the current window gate.
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
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

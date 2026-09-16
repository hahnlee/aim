package android.app;

import android.graphics.Rect;

public class WindowConfiguration {
    private Rect bounds = new Rect();
    private Rect appBounds = new Rect();
    private Rect maxBounds = new Rect();

    public WindowConfiguration() {}

    public WindowConfiguration(WindowConfiguration other) {
        bounds = new Rect(other.bounds);
        appBounds = new Rect(other.appBounds);
        maxBounds = new Rect(other.maxBounds);
    }

    public void setBounds(Rect value) {
        bounds = new Rect(value);
    }

    public void setAppBounds(Rect value) {
        appBounds = new Rect(value);
    }

    public void setMaxBounds(Rect value) {
        maxBounds = new Rect(value);
    }

    public Rect getBounds() {
        return new Rect(bounds);
    }

    public Rect getAppBounds() {
        return new Rect(appBounds);
    }

    public Rect getMaxBounds() {
        return new Rect(maxBounds);
    }
}

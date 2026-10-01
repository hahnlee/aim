// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Context;

public class Notification {
    public static final int DEFAULT_LIGHTS = 4;

    public Notification() { throw new RuntimeException("stub"); }

    public static class Builder {
        public Builder(Context context, String channelId) { throw new RuntimeException("stub"); }
        public Builder setWhen(long when) { throw new RuntimeException("stub"); }
        public Builder setSmallIcon(int icon) { throw new RuntimeException("stub"); }
        public Builder setContentTitle(CharSequence title) { throw new RuntimeException("stub"); }
        public Builder setContentText(CharSequence text) { throw new RuntimeException("stub"); }
        public Builder setContentIntent(PendingIntent intent) { throw new RuntimeException("stub"); }
        public Builder setOngoing(boolean ongoing) { throw new RuntimeException("stub"); }
        public Builder setDefaults(int defaults) { throw new RuntimeException("stub"); }
        public Builder setColor(int argb) { throw new RuntimeException("stub"); }
        public Notification build() { throw new RuntimeException("stub"); }
    }
}

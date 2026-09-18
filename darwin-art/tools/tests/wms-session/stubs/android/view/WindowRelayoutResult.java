package android.view;

import android.graphics.Rect;
import android.content.res.Configuration;
import android.os.Parcel;
import android.os.Parcelable;

public class WindowRelayoutResult implements Parcelable {
    public final Frames frames = new Frames();
    public Configuration mergedConfiguration = new Configuration();
    public SurfaceControl surfaceControl = new SurfaceControl();
    public int syncSeqId;
    public static final class Frames {
        public Rect frame = new Rect();
        public Rect displayFrame = new Rect();
        public Rect parentFrame = new Rect();
    }
    @Override public void writeToParcel(Parcel dest, int flags) {}
    @Override public int describeContents() { return 0; }
    public static final Parcelable.Creator<WindowRelayoutResult> CREATOR =
            new Parcelable.Creator<WindowRelayoutResult>() {
        public WindowRelayoutResult createFromParcel(Parcel source) { return new WindowRelayoutResult(); }
        public WindowRelayoutResult[] newArray(int size) { return new WindowRelayoutResult[size]; }
    };
}

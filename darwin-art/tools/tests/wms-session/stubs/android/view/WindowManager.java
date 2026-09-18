package android.view;

import android.os.Parcel;
import android.os.Parcelable;
import android.os.IBinder;

public interface WindowManager {
    class LayoutParams implements Parcelable {
        public static final int FIRST_APPLICATION_WINDOW = 1;
        public static final int LAST_APPLICATION_WINDOW = 99;
        public static final int FIRST_SUB_WINDOW = 1000;
        public static final int LAST_SUB_WINDOW = 1999;
        public int type;
        public int flags;
        public IBinder token;
        public int width = 100;
        public int height = 100;
        public int gravity;
        public int x;
        public int y;
        public CharSequence accessibilityTitle;

        public LayoutParams() {}
        public LayoutParams(LayoutParams other) { copyFrom(other); }
        public void copyFrom(LayoutParams other) {
            type=other.type; flags=other.flags; token=other.token; width=other.width;
            height=other.height; gravity=other.gravity;
            x=other.x; y=other.y; accessibilityTitle=other.accessibilityTitle;
        }
        @Override public void writeToParcel(Parcel dest, int flags) { dest.writeInt(type); }
        @Override public int describeContents() { return 0; }
        public static final Parcelable.Creator<LayoutParams> CREATOR =
                new Parcelable.Creator<LayoutParams>() {
            public LayoutParams createFromParcel(Parcel source) { LayoutParams p = new LayoutParams(); p.type=source.readInt(); return p; }
            public LayoutParams[] newArray(int size) { return new LayoutParams[size]; }
        };
    }
}

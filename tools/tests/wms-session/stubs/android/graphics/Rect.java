package android.graphics;

import android.os.Parcel;
import android.os.Parcelable;

public class Rect implements Parcelable {
    public int left, top, right, bottom;
    public Rect() {}
    public Rect(int left, int top, int right, int bottom) {
        this.left = left; this.top = top; this.right = right; this.bottom = bottom;
    }
    public Rect(Rect other) { this(other.left, other.top, other.right, other.bottom); }
    public int width() { return right - left; }
    public int height() { return bottom - top; }
    public boolean isEmpty() { return left >= right || top >= bottom; }
    public void set(Rect other) { left=other.left; top=other.top; right=other.right; bottom=other.bottom; }
    public void set(int left, int top, int right, int bottom) {
        this.left=left; this.top=top; this.right=right; this.bottom=bottom;
    }
    @Override public void writeToParcel(Parcel dest, int flags) {
        dest.writeInt(left); dest.writeInt(top); dest.writeInt(right); dest.writeInt(bottom);
    }
    @Override public int describeContents() { return 0; }
    public static final Parcelable.Creator<Rect> CREATOR = new Parcelable.Creator<Rect>() {
        public Rect createFromParcel(Parcel source) {
            return new Rect(source.readInt(), source.readInt(), source.readInt(), source.readInt());
        }
        public Rect[] newArray(int size) { return new Rect[size]; }
    };
}

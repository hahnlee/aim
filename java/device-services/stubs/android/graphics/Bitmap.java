// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.graphics;

public final class Bitmap implements android.os.Parcelable {
    static int getDefaultDensity() { throw new RuntimeException("stub"); }
    public static final android.os.Parcelable.Creator<Bitmap> CREATOR = null;
    public void writeToParcel(android.os.Parcel parcel, int flags) { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public enum CompressFormat { JPEG(0), PNG(1), WEBP(2), WEBP_LOSSY(3), WEBP_LOSSLESS(4); CompressFormat(int nativeInt){throw new RuntimeException("stub");} }
    public boolean compress(CompressFormat format, int quality, java.io.OutputStream stream) { throw new RuntimeException("stub"); }
    public static Bitmap createScaledBitmap(Bitmap source, int width, int height, boolean filter) { throw new RuntimeException("stub"); }
    Bitmap(long nativeBitmap, int width, int height, int density, boolean requestPremultiplied,
            byte[] ninePatchChunk, NinePatch.InsetStruct ninePatchInsets) {
        throw new RuntimeException("stub");
    }
    public int getDensity() { throw new RuntimeException("stub"); }
    public final int getWidth() { throw new RuntimeException("stub"); }
    public final int getHeight() { throw new RuntimeException("stub"); }
 public void recycle(){throw new RuntimeException("stub");}
 public enum Config {ARGB_8888(5); Config(int nativeInt){throw new RuntimeException("stub");}}
 public static Bitmap createBitmap(int width,int height,Config config){throw new RuntimeException("stub");}
}

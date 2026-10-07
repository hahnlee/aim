// A stub of the image's class for compiling against (docs/build.md, "Java"):
// Compile only; installer_parcel verifies linkage against the original boot classpath.
package android.graphics;

public final class Bitmap {
    Bitmap(long nativeBitmap, int width, int height, int density, boolean requestPremultiplied,
            byte[] ninePatchChunk, NinePatch.InsetStruct ninePatchInsets) {
        throw new RuntimeException("stub");
    }
    public final int getWidth() { throw new RuntimeException("stub"); }
    public final int getHeight() { throw new RuntimeException("stub"); }
public enum Config {ARGB_8888}
public static Bitmap createBitmap(int width,int height,Config config){throw new RuntimeException("stub");}
public void setPixel(int x,int y,int color){throw new RuntimeException("stub");}
public int getPixel(int x,int y){throw new RuntimeException("stub");}
public void setDensity(int density){throw new RuntimeException("stub");}
public void setGainmap(Gainmap gainmap){throw new RuntimeException("stub");}
public boolean hasGainmap(){throw new RuntimeException("stub");}
public Gainmap getGainmap(){throw new RuntimeException("stub");}
}

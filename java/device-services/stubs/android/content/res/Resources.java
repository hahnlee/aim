// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.res;

public class Resources {
    public Resources(AssetManager assets,android.util.DisplayMetrics metrics,Configuration configuration){throw new RuntimeException("stub");}

    public Resources(ClassLoader classLoader) { throw new RuntimeException("stub"); }
    public static Resources getSystem() { throw new RuntimeException("stub"); }
    public CharSequence getText(int id) { throw new RuntimeException("stub"); }
    public XmlResourceParser getXml(int id) { throw new RuntimeException("stub"); }
    public String getString(int id) { throw new RuntimeException("stub"); }
    public int getIdentifier(String name, String defType, String defPackage) { throw new RuntimeException("stub"); }
    public boolean getBoolean(int id) { throw new RuntimeException("stub"); }
    public String[] getStringArray(int id) { throw new RuntimeException("stub"); }
    public int getInteger(int id) { throw new RuntimeException("stub"); }
    public Configuration getConfiguration() { throw new RuntimeException("stub"); }
    public int getDimensionPixelSize(int id) { throw new RuntimeException("stub"); }
    public float getFloat(int id) { throw new RuntimeException("stub"); }
    public android.util.DisplayMetrics getDisplayMetrics() { throw new RuntimeException("stub"); }

    public final class Theme {
        private Theme() { throw new RuntimeException("stub"); }
    }
 public android.graphics.drawable.Drawable getDrawable(int id,Theme theme)throws NotFoundException{throw new RuntimeException("stub");}
    public static class NotFoundException extends RuntimeException {
        public NotFoundException() { throw new RuntimeException("stub"); }
        public NotFoundException(String message) { super(message); throw new RuntimeException("stub"); }
    }
}

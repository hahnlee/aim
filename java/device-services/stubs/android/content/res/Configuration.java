// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.res;

public final class Configuration {
    public static final int UI_MODE_NIGHT_MASK = 0x30;
    public static final int UI_MODE_NIGHT_YES = 0x20;

    public int uiMode;
    public int densityDpi;
    public int mcc; public int mnc; public int orientation; public int touchscreen;
    public int keyboard; public int navigation; public int keyboardHidden; public int hardKeyboardHidden;
    public int screenLayout; public int smallestScreenWidthDp; public int screenWidthDp; public int screenHeightDp; public int colorMode;
    public android.os.LocaleList getLocales() { throw new RuntimeException("stub"); }
    public int getGrammaticalGender() { throw new RuntimeException("stub"); }
    public int seq;
    public final android.app.WindowConfiguration windowConfiguration = null;

    public Configuration() { throw new RuntimeException("stub"); }
    public void setToDefaults() { throw new RuntimeException("stub"); }
    public void setLocale(java.util.Locale locale) { throw new RuntimeException("stub"); }
}

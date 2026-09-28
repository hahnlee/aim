package android.util;
import org.xmlpull.v1.XmlPullParser;
import org.kxml2.io.KXmlParser;
// Test-only Android factory boundary; actual pull parser from the installed SDK.
public final class Xml {
    public static XmlPullParser newPullParser() {
        return new KXmlParser();
    }
}

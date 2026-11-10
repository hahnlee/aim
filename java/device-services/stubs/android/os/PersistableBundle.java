// Compile-only pinned image API; checked against the original image.
package android.os;
public final class PersistableBundle extends BaseBundle implements android.os.Parcelable {
    public static PersistableBundle restoreFromXml(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException {
        throw new RuntimeException("stub");
    }
    public void saveToXml(com.android.modules.utils.TypedXmlSerializer out) throws java.io.IOException, org.xmlpull.v1.XmlPullParserException {
        throw new RuntimeException("stub");
    }

 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel dest,int flags){throw new RuntimeException("stub");}
}

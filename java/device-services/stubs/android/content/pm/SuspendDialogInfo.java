// Compile-only pinned image API; checked against the original image.
package android.content.pm;
public final class SuspendDialogInfo {
    public static final class Builder {
        public Builder() { throw new RuntimeException("stub"); }
        public Builder setIcon(int id) { throw new RuntimeException("stub"); }
        public Builder setTitle(int id) { throw new RuntimeException("stub"); }
        public Builder setTitle(String title) { throw new RuntimeException("stub"); }
        public Builder setMessage(int id) { throw new RuntimeException("stub"); }
        public Builder setMessage(String message) { throw new RuntimeException("stub"); }
        public Builder setNeutralButtonText(int id) { throw new RuntimeException("stub"); }
        public Builder setNeutralButtonText(String text) { throw new RuntimeException("stub"); }
        public Builder setNeutralButtonAction(int action) { throw new RuntimeException("stub"); }
        public SuspendDialogInfo build() { throw new RuntimeException("stub"); }
    }
    private SuspendDialogInfo(android.os.Parcel source) { throw new RuntimeException("stub"); }
    public static SuspendDialogInfo restoreFromXml(com.android.modules.utils.TypedXmlPullParser parser) {
        throw new RuntimeException("stub");
    }
    public void saveToXml(com.android.modules.utils.TypedXmlSerializer serializer) throws java.io.IOException {
        throw new RuntimeException("stub");
    }
    public int getIconResId() { throw new RuntimeException("stub"); }
    public int getTitleResId() { throw new RuntimeException("stub"); }
    public String getTitle() { throw new RuntimeException("stub"); }
    public int getDialogMessageResId() { throw new RuntimeException("stub"); }
    public String getDialogMessage() { throw new RuntimeException("stub"); }
    public int getNeutralButtonTextResId() { throw new RuntimeException("stub"); }
    public String getNeutralButtonText() { throw new RuntimeException("stub"); }
    public int getNeutralButtonAction() { throw new RuntimeException("stub"); }
}

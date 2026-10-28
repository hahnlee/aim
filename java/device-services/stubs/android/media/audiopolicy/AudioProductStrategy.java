// Compile-only pinned image API, android-16.0.0_r1; checked by image linkage.
package android.media.audiopolicy;
public class AudioProductStrategy implements android.os.Parcelable {
    private AudioProductStrategy(String name, int id, AudioAttributesGroup[] groups) {}
    public static class AudioAttributesGroup implements android.os.Parcelable {
        AudioAttributesGroup(int group, int stream, android.media.AudioAttributes[] attributes) {}
        public int describeContents() { throw new RuntimeException("stub"); }
        public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
    }
    public static java.util.List<AudioProductStrategy> getAudioProductStrategies() { throw new RuntimeException("stub"); }
    public int describeContents() { throw new RuntimeException("stub"); }
    public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException("stub"); }
}

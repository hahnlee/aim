package android.media;
import java.util.*;
import android.os.Binder;
import android.media.audiopolicy.AudioProductStrategy;
public class AudioSystem {
    public static boolean fail;
    public static int getNumStreamTypes() { return 12; }
    public static ArrayList<AudioDeviceAttributes> getDevicesForAttributes(AudioAttributes a, boolean volume) {
        if (a != AudioProductStrategy.attributes || !volume || Binder.cleared != 1) throw new AssertionError();
        if (fail) throw new IllegalStateException("policy failure");
        return new ArrayList<>();
    }
    public static Set<Integer> generateAudioDeviceTypesSet(List<AudioDeviceAttributes> devices) { return Set.of(2, 1024); }
    public static int getDeviceMaskFromSet(Set<Integer> devices) { return devices.stream().reduce(0, (a,b) -> a | b); }
    public static boolean isBluetoothDevice(int device) { return false; }
}

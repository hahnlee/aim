package dev.darwinart.runtime.power;

import android.content.Intent;
import dev.darwinart.runtime.am.SystemBroadcasts;
import java.util.ArrayList;
import java.util.List;

/** Host power-source readings become BatteryService's sticky and transition broadcasts. */
public final class BatteryServiceTest {
    // android.os.BatteryManager constants.
    private static final int STATUS_UNKNOWN = 1;
    private static final int STATUS_CHARGING = 2;
    private static final int STATUS_DISCHARGING = 3;
    private static final int STATUS_NOT_CHARGING = 4;
    private static final int STATUS_FULL = 5;
    private static final int PLUGGED_AC = 1;

    private static final class Recorder implements SystemBroadcasts {
        final List<Intent> intents = new ArrayList<>();
        final List<Boolean> sticky = new ArrayList<>();

        @Override public void broadcastAsSystem(Intent intent, boolean isSticky) {
            intents.add(intent);
            sticky.add(isSticky);
        }
    }

    private static BatteryStateProvider.Reading reading(boolean present, int level,
            boolean charging, boolean charged, boolean ac) {
        return new BatteryStateProvider.Reading(present, level, 100, charging, charged, ac);
    }

    public static void main(String[] args) {
        statusFollowsTheHostSource();
        transitionsBroadcastOnce();
        System.out.println("BatteryServiceTest PASS");
    }

    private static void statusFollowsTheHostSource() {
        BatteryService.State desktop =
                new BatteryService.State(reading(false, 0, false, false, true));
        check(!desktop.present && desktop.level == 100 && desktop.status == STATUS_UNKNOWN
                && desktop.plugged == PLUGGED_AC && !desktop.low(),
                "a host without a battery reports a full, AC-powered, absent battery");
        check(new BatteryService.State(reading(true, 80, false, false, true)).status
                == STATUS_NOT_CHARGING, "AC without charging is NOT_CHARGING");
        check(new BatteryService.State(reading(true, 50, true, false, true)).status
                == STATUS_CHARGING, "charging");
        check(new BatteryService.State(reading(true, 100, false, true, true)).status
                == STATUS_FULL, "charged");
        BatteryService.State discharging =
                new BatteryService.State(reading(true, 15, false, false, false));
        check(discharging.status == STATUS_DISCHARGING && discharging.low(),
                "15% on battery is low");
        Intent intent = BatteryService.batteryChanged(discharging, 7);
        check(Intent.ACTION_BATTERY_CHANGED.equals(intent.getAction())
                && intent.getIntExtra("level", -1) == 15 && intent.getIntExtra("scale", -1) == 100
                && intent.getIntExtra("status", -1) == STATUS_DISCHARGING
                && intent.getIntExtra("plugged", -1) == 0
                && intent.getBooleanExtra("battery_low", false)
                && intent.getIntExtra("seq", -1) == 7
                && (intent.getFlags() & Intent.FLAG_RECEIVER_REGISTERED_ONLY) != 0,
                "ACTION_BATTERY_CHANGED carries BatteryService extras");
    }

    private static void transitionsBroadcastOnce() {
        final BatteryStateProvider.Reading[] current = {reading(true, 80, false, false, true)};
        Recorder recorder = new Recorder();
        BatteryService service = new BatteryService(() -> current[0], recorder);
        service.update();
        check(recorder.intents.size() == 1 && recorder.sticky.get(0),
                "the first reading publishes the sticky");
        service.update();
        check(recorder.intents.size() == 1, "an unchanged reading is not rebroadcast");
        current[0] = reading(true, 18, false, false, false);
        service.update();
        check(actions(recorder).equals(java.util.Arrays.asList(Intent.ACTION_BATTERY_CHANGED,
                Intent.ACTION_BATTERY_CHANGED, Intent.ACTION_POWER_DISCONNECTED,
                Intent.ACTION_BATTERY_LOW)), "unplug below the warning level: " + actions(recorder));
        current[0] = reading(true, 18, true, false, true);
        service.update();
        check(actions(recorder).subList(4, 7).equals(java.util.Arrays.asList(
                Intent.ACTION_BATTERY_CHANGED, Intent.ACTION_POWER_CONNECTED,
                Intent.ACTION_BATTERY_OKAY)), "plugging in clears the low warning");
    }

    private static List<String> actions(Recorder recorder) {
        List<String> result = new ArrayList<>();
        for (Intent intent : recorder.intents) result.add(intent.getAction());
        return result;
    }

    private static void check(boolean condition, String message) {
        if (!condition) throw new AssertionError(message);
    }
}

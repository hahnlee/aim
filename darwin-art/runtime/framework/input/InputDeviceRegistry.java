package dev.darwinart.runtime.input;

import android.os.Parcelable;
import android.view.InputDevice;
import java.lang.reflect.Method;

/** System-owned Android input-device inventory for the macOS seat. */
public final class InputDeviceRegistry {
    public static final int PHYSICAL_KEYBOARD_ID = 1;
    public static final int VIRTUAL_KEYBOARD_ID = -1;

    private final Parcelable physicalKeyboard;
    private final Parcelable virtualKeyboard;

    public InputDeviceRegistry() {
        physicalKeyboard = createKeyboard(PHYSICAL_KEYBOARD_ID,
                "Mac Physical Keyboard", "darwin:keyboard:primary", true);
        virtualKeyboard = createKeyboard(VIRTUAL_KEYBOARD_ID,
                "Virtual", "virtual", false);
    }

    public Parcelable inputDevice(int id) {
        if (id == PHYSICAL_KEYBOARD_ID) return physicalKeyboard;
        if (id == VIRTUAL_KEYBOARD_ID) return virtualKeyboard;
        return null;
    }

    public int[] inputDeviceIds() {
        // InputManagerGlobal consults this inventory before issuing the
        // getInputDevice call. Android therefore keeps its virtual fallback
        // discoverable here alongside connected physical devices.
        return new int[] { VIRTUAL_KEYBOARD_ID, PHYSICAL_KEYBOARD_ID };
    }

    private static Parcelable createKeyboard(
            int id, String name, String descriptor, boolean external) {
        try {
            Class<?> keyMapClass = Class.forName("android.view.KeyCharacterMap");
            Object keyMap = SystemKeyboardMaps.load(id);
            Class<?> builderClass = Class.forName("android.view.InputDevice$Builder");
            Object builder = builderClass.getDeclaredConstructor().newInstance();
            invoke(builderClass, builder, "setId", int.class, Integer.valueOf(id));
            invoke(builderClass, builder, "setGeneration", int.class, Integer.valueOf(1));
            invoke(builderClass, builder, "setName", String.class, name);
            invoke(builderClass, builder, "setDescriptor", String.class, descriptor);
            invoke(builderClass, builder, "setExternal", boolean.class, Boolean.valueOf(external));
            invoke(builderClass, builder, "setSources", int.class, Integer.valueOf(0x101));
            invoke(builderClass, builder, "setKeyboardType", int.class,
                    Integer.valueOf(InputDevice.KEYBOARD_TYPE_ALPHABETIC));
            invoke(builderClass, builder, "setKeyCharacterMap", keyMapClass, keyMap);
            invoke(builderClass, builder, "setEnabled", boolean.class, Boolean.TRUE);
            return (Parcelable) builderClass.getMethod("build").invoke(builder);
        } catch (ReflectiveOperationException error) {
            throw new IllegalStateException("Android InputDevice contract changed", error);
        }
    }

    private static void invoke(Class<?> type, Object receiver, String name,
            Class<?> argumentType, Object value) throws ReflectiveOperationException {
        Method method = type.getMethod(name, argumentType);
        method.invoke(receiver, value);
    }
}

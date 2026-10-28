package android.provider;
public final class DeviceConfig { public static String flag="0"; public static String getString(String ns,String key,String fallback){if(!ns.equals("tethering")||!key.equals("allow_sysui_connectivity_reports"))throw new AssertionError(); return flag;} }

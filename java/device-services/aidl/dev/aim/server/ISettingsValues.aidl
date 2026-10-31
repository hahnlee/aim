package dev.aim.server;

/** Original system-settings value conversions; system UID only. */
interface ISettingsValues {
    String getAsString(in byte[] value);
    String bestPersistentLocale(String tags);
}

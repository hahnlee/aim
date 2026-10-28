package dev.aim.server;

/** ContentValues.getAsString's original scalar conversion; system UID only. */
interface ISettingsValues {
    String getAsString(in byte[] value);
}

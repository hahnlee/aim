package dev.aim.server;

/** Binder transport is supplied by the Rust host fixture. */
public interface IPackageIntentParser {
    byte[] parseIntent(String[] arguments);
    byte[] parseBroadcast(String[] arguments);
    String formatBroadcastResult(byte[] result);
    abstract class Stub implements IPackageIntentParser {}
}

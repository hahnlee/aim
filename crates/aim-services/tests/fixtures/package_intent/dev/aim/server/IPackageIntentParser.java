package dev.aim.server;

/** Binder transport is supplied by the Rust host fixture. */
public interface IPackageIntentParser {
    byte[] parseIntent(String[] arguments);
    abstract class Stub implements IPackageIntentParser {}
}

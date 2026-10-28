package dev.aim.server;
interface IProviderQueries {
    abstract class Stub extends android.os.Binder {
        public abstract boolean isCloneRedirected(String authority, int uid, int user);
    }
}

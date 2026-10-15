// Compile-only original optional service owner.
package com.android.server.apphibernation;
public abstract class AppHibernationManagerInternal {
    public abstract boolean isHibernatingForUser(String name, int user);
    public abstract void setHibernatingForUser(String name, int user, boolean value);
    public abstract void setHibernatingGlobally(String name, boolean value);
 public abstract boolean isHibernatingGlobally(String name);
 public abstract boolean isOatArtifactDeletionEnabled();
}

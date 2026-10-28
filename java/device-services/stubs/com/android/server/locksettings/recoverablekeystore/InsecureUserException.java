// Compile-only pinned image API; never packaged with the guest.
package com.android.server.locksettings.recoverablekeystore;
public class InsecureUserException extends Exception { private InsecureUserException(String message) { throw new RuntimeException("stub"); } }

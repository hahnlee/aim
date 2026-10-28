package android.os;
public class RemoteException extends Exception { public RuntimeException rethrowFromSystemServer(){return new IllegalStateException(this);} }

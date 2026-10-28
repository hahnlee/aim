package android.os;public class ServiceSpecificException extends RuntimeException {public final int errorCode;public ServiceSpecificException(int code){errorCode=code;}}

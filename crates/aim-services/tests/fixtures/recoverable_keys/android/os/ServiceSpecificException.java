// Host-only Android/owner boundary double; production bridge compiled unchanged.
package android.os;
public class ServiceSpecificException extends RuntimeException {
 public final int errorCode;
 public ServiceSpecificException(int code,String message){super(message);errorCode=code;}
}

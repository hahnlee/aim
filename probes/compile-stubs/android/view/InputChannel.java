package android.view;

import android.os.IBinder;
import android.os.Parcel;
import android.os.ParcelFileDescriptor;
import android.os.Parcelable;
import java.io.IOException;

/** Minimal framework-compatible endpoint used by the host input transport. */
public final class InputChannel implements Parcelable {
  private final String name;
  private final IBinder token;
  private final ParcelFileDescriptor endpoint;

  private InputChannel(String name, IBinder token, ParcelFileDescriptor endpoint) {
    this.name = name;
    this.token = token;
    this.endpoint = endpoint;
  }

  public static InputChannel[] openInputChannelPair(String name) throws IOException {
    ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createPipe();
    IBinder token = new android.os.Binder();
    return new InputChannel[] {
        new InputChannel(name, token, pipe[0]),
        new InputChannel(name, token, pipe[1])
    };
  }

  public String getName() { return name; }
  public IBinder getToken() { return token; }

  public void dispose() {
    if (endpoint != null) {
      try { endpoint.close(); } catch (IOException ignored) { }
    }
  }

  @Override public int describeContents() { return CONTENTS_FILE_DESCRIPTOR; }
  @Override public void writeToParcel(Parcel out, int flags) {
    out.writeString(name);
    out.writeStrongBinder(token);
    out.writeParcelable(endpoint, flags);
  }

  public static final Creator<InputChannel> CREATOR = new Creator<InputChannel>() {
    @Override public InputChannel createFromParcel(Parcel in) {
      String name = in.readString();
      IBinder token = in.readStrongBinder();
      ParcelFileDescriptor endpoint = in.readParcelable(
          InputChannel.class.getClassLoader());
      return new InputChannel(name, token, endpoint);
    }
    @Override public InputChannel[] newArray(int size) {
      return new InputChannel[size];
    }
  };
}

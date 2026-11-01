import android.os.Parcel;
import android.view.SurfaceControl;
import java.nio.file.Files;
import java.nio.file.Path;

/** Original JNI/libgui oracle; restore live Binder objects before the native read. */
public final class SurfaceParcelOracle {
    public static void main(String[] args) throws Exception {
        byte[] bytes = Files.readAllBytes(Path.of(args[0]));
        Parcel input = Parcel.obtain(), output = Parcel.obtain();
        input.unmarshall(bytes, 0, bytes.length);
        String[] positions = new String(Files.readAllBytes(Path.of(args[0] + ".offsets")), "US-ASCII").split(",");
        for (String at : positions) {
            input.setDataPosition(Integer.parseInt(at));
            input.writeStrongBinder(new android.os.Binder());
        }
        input.setDataPosition(0);
        try (SurfaceControl.Transaction t = SurfaceControl.Transaction.CREATOR.createFromParcel(input)) {
            if (input.readInt() != 0x13572468 || input.dataPosition() != bytes.length)
                throw new AssertionError("native reader did not consume the full transaction");
            t.writeToParcel(output, 0);
            output.setDataPosition(Integer.parseInt(positions[positions.length - 1]) + 28);
            if (output.readInt() != 71 || !"parent".equals(output.readString()) || output.readInt() != 3
                    || output.readInt() != 0 || output.readInt() != 0 || output.readInt() != -3)
                throw new AssertionError("native round trip lost parent SurfaceControl");
        }
        input.recycle(); output.recycle();
        System.out.println("ok original native reparent parcel");
    }
}

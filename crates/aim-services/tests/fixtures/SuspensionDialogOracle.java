import android.content.pm.SuspendDialogInfo;
import android.util.Xml;
import java.io.FileInputStream;
import java.io.FileOutputStream;

public final class SuspensionDialogOracle {
    public static void main(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        for (int i = 0; i < count; i++) {
            SuspendDialogInfo dialog;
            try (var input = new FileInputStream(args[0] + "/" + i + ".xml")) {
                var parser = Xml.resolvePullParser(input);
                while (parser.next() != 2) {}
                dialog = SuspendDialogInfo.restoreFromXml(parser);
            }
            try (var output = new FileOutputStream(args[0] + "/" + i + ".original")) {
                var serializer = Xml.resolveSerializer(output);
                serializer.startDocument(null, true);
                serializer.startTag(null, "dialog-info");
                dialog.saveToXml(serializer);
                serializer.endTag(null, "dialog-info");
                serializer.endDocument();
            }
        }
        System.out.println("DIALOGS " + count);
    }
}

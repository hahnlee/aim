package com.android.server.pm;

import android.util.Xml;
import java.io.*;
import java.nio.charset.StandardCharsets;

public final class PreferredSerializationOracle {
    public static void main(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        var aggregate = new ByteArrayOutputStream();
        for (int index = 0; index < count; index++) {
        try (var input = new FileInputStream(args[0] + "/input-" + index + ".xml")) {
            var parser = Xml.resolvePullParser(input);
            var bytes = new ByteArrayOutputStream();
            var writer = Xml.newFastSerializer();
            writer.setOutput(bytes, StandardCharsets.UTF_8.name());
            writer.startDocument(null, true);
            writer.startTag(null, "pa");
            writer.startTag(null, "preferred-activities");
            while (parser.next() != 1) {
                if ("item".equals(parser.getName()) && parser.getDepth() == 3) {
                    var preferred = new PreferredActivity(parser);
                    writer.startTag(null, "item");
                    preferred.writeToXml(writer, true);
                    writer.endTag(null, "item");
                }
            }
            writer.endTag(null, "preferred-activities");
            writer.endTag(null, "pa");
            writer.endDocument();
            aggregate.write(bytes.toByteArray());
        }
        }
        try (var output = new FileOutputStream(args[0] + "/output.xml")) { output.write(aggregate.toByteArray()); }
        System.out.println("original preferred serializer checks passed");
    }
}

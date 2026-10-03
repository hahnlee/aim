package com.android.server.pm;

public final class PreferredClearingOracle {
    public static void main(String[] args) throws Exception {
        Settings settings = new Settings(java.util.Map.of());
        try (var input = new java.io.FileInputStream(args[0])) {
            var parser = android.util.Xml.resolvePullParser(input);
            while (!"preferred-activities".equals(parser.getName())) parser.next();
            settings.readPreferredActivitiesLPw(parser, 0);
        }
        for (String name : new String[] {"removed", "removed", null}) {
            var changed = new android.util.SparseBooleanArray();
            settings.clearPackagePreferredActivities(name, changed, 0);
            var output = new java.io.ByteArrayOutputStream();
            var serializer = android.util.Xml.resolveSerializer(output);
            serializer.startDocument(null, true);
            settings.writePreferredActivitiesLPr(serializer, 0, true);
            serializer.endDocument();
            var parser = android.util.Xml.resolvePullParser(
                new java.io.ByteArrayInputStream(output.toByteArray()));
            var choices = new java.util.ArrayList<String>();
            int event;
            while ((event = parser.next()) != 1) {
                if (event == 2 && "item".equals(parser.getName())) {
                    String choice = parser.getAttributeValue(null, "name");
                    if (choice != null) choices.add(choice);
                }
            }
            java.util.Collections.sort(choices);
            System.out.println(changed.get(0) + " " + String.join(",", choices));
        }
        // Settings starts BackgroundThread; this isolated oracle owns the process.
        System.exit(0);
    }
}

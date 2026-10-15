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
        Settings all = new Settings(java.util.Map.of());
        int[] users = {0, 10, 20, 30};
        String[] files = {args[1], args[0], args[1], args[2]};
        for (int i = 0; i < users.length; i++) {
            try (var input = new java.io.FileInputStream(files[i])) {
                var parser = android.util.Xml.resolvePullParser(input);
                while (!"preferred-activities".equals(parser.getName())) parser.next();
                all.readPreferredActivitiesLPw(parser, users[i]);
            }
        }
        for (String name : new String[] {"removed", "removed", null, null}) {
            var changed = new android.util.SparseBooleanArray();
            all.clearPackagePreferredActivities(name, changed, -1);
            var ids = new java.util.ArrayList<String>();
            for (int id : users) {
                if (changed.get(id)) ids.add(Integer.toString(id));
            }
            System.out.println("all " + String.join(",", ids));
        }
        // Settings starts BackgroundThread; this isolated oracle owns the process.
        System.exit(0);
    }
}

package com.android.server.devicepolicy;
public class PolicyState<V> {
    public static Boolean resolved;
    public static boolean admins;
    public static int reads;
    static <V> PolicyState<V> readFromXml(PolicyDefinition<V> definition, com.android.modules.utils.TypedXmlPullParser parser)
            throws java.io.IOException, org.xmlpull.v1.XmlPullParserException {
        reads++;
        com.android.internal.util.XmlUtils.skipCurrentTag(parser);
        return new PolicyState<V>();
    }
    android.app.admin.PolicyValue<Boolean> getCurrentResolvedPolicy() {
        return resolved == null ? null : new android.app.admin.PolicyValue<Boolean>(resolved) {
            @Override public Boolean getValue() { return resolved; }
        };
    }
    java.util.LinkedHashMap<String, Boolean> getPoliciesSetByAdmins() {
        var result = new java.util.LinkedHashMap<String, Boolean>();
        if (admins) result.put("admin", resolved);
        return result;
    }
}

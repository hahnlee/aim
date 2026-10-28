package com.android.server.devicepolicy;
class PolicyState<V> {
    private PolicyState(PolicyDefinition<V> definition) {}
    static <V> PolicyState<V> readFromXml(PolicyDefinition<V> definition, com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException, org.xmlpull.v1.XmlPullParserException { throw new RuntimeException("stub"); }
    android.app.admin.PolicyValue<V> getCurrentResolvedPolicy() { throw new RuntimeException("stub"); }
    java.util.LinkedHashMap<?, android.app.admin.PolicyValue<V>> getPoliciesSetByAdmins() { throw new RuntimeException("stub"); }
}

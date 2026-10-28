package com.android.server.devicepolicy;
class PolicyDefinition<V> {
    private PolicyDefinition(android.app.admin.PolicyKey key, ResolutionMechanism<V> resolution,
            int flags, com.android.internal.util.function.QuadFunction<?, ?, ?, ?, ?> enforcer,
            PolicySerializer<V> serializer) {}
    static PolicyDefinition<?> readFromXml(com.android.modules.utils.TypedXmlPullParser parser) throws org.xmlpull.v1.XmlPullParserException { throw new RuntimeException("stub"); }
    android.app.admin.PolicyKey getPolicyKey() { throw new RuntimeException("stub"); }
}

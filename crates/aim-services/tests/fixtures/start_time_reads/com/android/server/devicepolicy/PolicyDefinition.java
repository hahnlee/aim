package com.android.server.devicepolicy;
class PolicyDefinition<V> {
    private final String identifier;
    private PolicyDefinition(String identifier) { this.identifier = identifier; }
    static PolicyDefinition<?> readFromXml(com.android.modules.utils.TypedXmlPullParser parser) {
        return new PolicyDefinition<Boolean>(parser.getAttributeValue(null, "policy-identifier"));
    }
    android.app.admin.PolicyKey getPolicyKey() {
        return new android.app.admin.PolicyKey(identifier) {
            @Override public String getIdentifier() { return identifier; }
        };
    }
}

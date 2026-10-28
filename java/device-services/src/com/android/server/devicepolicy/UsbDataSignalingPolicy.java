package com.android.server.devicepolicy;

import android.util.Xml;
import com.android.internal.util.XmlUtils;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import org.xmlpull.v1.XmlPullParserException;

/** DevicePolicyEngine's global-only USB read, android-16.0.0_r1.
 * Policy definitions, admin decoding and resolved values remain original Java.
 * Copyright (C) The Android Open Source Project, Apache License 2.0. */
public final class UsbDataSignalingPolicy {
    private UsbDataSignalingPolicy() {}

    public static boolean isEnabled(byte[] policies) {
        if (policies.length == 0) return true;
        Boolean enabled = null;
        try {
            var parser = Xml.resolvePullParser(new ByteArrayInputStream(policies));
            int depth = parser.getDepth();
            while (XmlUtils.nextElementWithin(parser, depth)) {
                if (!"global-policy-entry".equals(parser.getName())) {
                    XmlUtils.skipCurrentTag(parser);
                    continue;
                }
                PolicyDefinition<?> definition = null;
                int entryDepth = parser.getDepth();
                while (XmlUtils.nextElementWithin(parser, entryDepth)) {
                    if ("policy-key-entry".equals(parser.getName())) {
                        definition = PolicyDefinition.readFromXml(parser);
                    } else if ("policy-state-entry".equals(parser.getName())
                            && definition != null
                            && "usbDataSignaling".equals(definition.getPolicyKey().getIdentifier())) {
                        var state = PolicyState.readFromXml(definition, parser);
                        var value = state.getCurrentResolvedPolicy();
                        enabled = state.getPoliciesSetByAdmins().isEmpty() || value == null
                                ? null : (Boolean) value.getValue();
                    } else {
                        XmlUtils.skipCurrentTag(parser);
                    }
                }
            }
        } catch (IOException | XmlPullParserException error) {
            throw new IllegalStateException("Cannot read USB data signaling policy", error);
        }
        return enabled == null || enabled;
    }
}

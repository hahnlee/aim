package dev.aim.probe;

import android.content.SharedPreferences;
import android.util.Log;
import android.util.Xml;

import org.xmlpull.v1.XmlPullParser;
import org.xmlpull.v1.XmlSerializer;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.lang.reflect.Method;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

/** Android-compatible, process-owned SharedPreferences backed by an atomic XML file. */
public final class ProbeSharedPreferences implements SharedPreferences {
    private static final Object REMOVED = new Object();
    private final File file;
    private final File backupFile;
    private final Map<String, Object> values = new HashMap<>();
    private final Set<OnSharedPreferenceChangeListener> listeners = new HashSet<>();

    public ProbeSharedPreferences(File file) {
        this.file = file;
        this.backupFile = new File(file.getPath() + ".bak");
        loadFromDisk();
    }

    @Override
    public synchronized Map<String, ?> getAll() {
        Map<String, Object> result = new HashMap<>();
        for (Map.Entry<String, Object> entry : values.entrySet()) {
            Object value = entry.getValue();
            result.put(entry.getKey(), value instanceof Set ? new HashSet<>((Set<?>) value) : value);
        }
        return Collections.unmodifiableMap(result);
    }

    @Override
    public synchronized String getString(String key, String defaultValue) {
        Object value = values.get(key);
        return value instanceof String ? (String) value : defaultValue;
    }

    @Override
    @SuppressWarnings("unchecked")
    public synchronized Set<String> getStringSet(String key, Set<String> defaultValue) {
        Object value = values.get(key);
        return value instanceof Set ? new HashSet<>((Set<String>) value)
                : defaultValue == null ? null : new HashSet<>(defaultValue);
    }

    @Override
    public synchronized int getInt(String key, int defaultValue) {
        Object value = values.get(key);
        return value instanceof Integer ? (Integer) value : defaultValue;
    }

    @Override
    public synchronized long getLong(String key, long defaultValue) {
        Object value = values.get(key);
        return value instanceof Long ? (Long) value : defaultValue;
    }

    @Override
    public synchronized float getFloat(String key, float defaultValue) {
        Object value = values.get(key);
        return value instanceof Float ? (Float) value : defaultValue;
    }

    @Override
    public synchronized boolean getBoolean(String key, boolean defaultValue) {
        Object value = values.get(key);
        return value instanceof Boolean ? (Boolean) value : defaultValue;
    }

    @Override
    public synchronized boolean contains(String key) {
        return values.containsKey(key);
    }

    @Override
    public Editor edit() {
        return new EditorImpl();
    }

    @Override
    public synchronized void registerOnSharedPreferenceChangeListener(
            OnSharedPreferenceChangeListener listener) {
        if (listener != null) listeners.add(listener);
    }

    @Override
    public synchronized void unregisterOnSharedPreferenceChangeListener(
            OnSharedPreferenceChangeListener listener) {
        listeners.remove(listener);
    }

    private final class EditorImpl implements Editor {
        private final Map<String, Object> changes = new HashMap<>();
        private boolean clear;

        @Override
        public Editor putString(String key, String value) {
            changes.put(key, value == null ? REMOVED : value);
            return this;
        }

        @Override
        public Editor putStringSet(String key, Set<String> value) {
            changes.put(key, value == null ? REMOVED : new HashSet<>(value));
            return this;
        }

        @Override
        public Editor putInt(String key, int value) {
            changes.put(key, value);
            return this;
        }

        @Override
        public Editor putLong(String key, long value) {
            changes.put(key, value);
            return this;
        }

        @Override
        public Editor putFloat(String key, float value) {
            changes.put(key, value);
            return this;
        }

        @Override
        public Editor putBoolean(String key, boolean value) {
            changes.put(key, value);
            return this;
        }

        @Override
        public Editor remove(String key) {
            changes.put(key, REMOVED);
            return this;
        }

        @Override
        public Editor clear() {
            clear = true;
            return this;
        }

        @Override
        public boolean commit() {
            return applyChanges();
        }

        @Override
        public void apply() {
            applyChanges();
        }

        private boolean applyChanges() {
            Set<String> changed = new HashSet<>();
            Set<OnSharedPreferenceChangeListener> observers;
            boolean written;
            synchronized (ProbeSharedPreferences.this) {
                Map<String, Object> before = new HashMap<>(values);
                if (clear) {
                    changed.addAll(values.keySet());
                    values.clear();
                }
                for (Map.Entry<String, Object> entry : changes.entrySet()) {
                    changed.add(entry.getKey());
                    if (entry.getValue() == REMOVED) {
                        values.remove(entry.getKey());
                    } else {
                        values.put(entry.getKey(), entry.getValue());
                    }
                }
                written = writeToDisk();
                if (!written) {
                    values.clear();
                    values.putAll(before);
                    changed.clear();
                }
                observers = new HashSet<>(listeners);
            }
            for (OnSharedPreferenceChangeListener listener : observers) {
                for (String key : changed) {
                    listener.onSharedPreferenceChanged(ProbeSharedPreferences.this, key);
                }
            }
            return written;
        }
    }

    private void loadFromDisk() {
        if (backupFile.exists()) {
            if (file.exists()) file.delete();
            backupFile.renameTo(file);
        }
        if (!file.exists()) return;
        FileInputStream input = null;
        try {
            input = new FileInputStream(file);
            XmlPullParser parser = resolvePullParser(input);
            String setKey = null;
            Set<String> setValue = null;
            for (int event = parser.getEventType(); event != XmlPullParser.END_DOCUMENT;
                    event = parser.next()) {
                if (event == XmlPullParser.START_TAG) {
                    String tag = parser.getName();
                    String key = parser.getAttributeValue(null, "name");
                    String encoded = parser.getAttributeValue(null, "value");
                    if ("set".equals(tag)) {
                        setKey = key;
                        setValue = new HashSet<>();
                    } else if ("string".equals(tag)) {
                        String text = parser.nextText();
                        if (setValue != null) setValue.add(text);
                        else if (key != null) values.put(key, text);
                    } else if (key != null && "int".equals(tag)) {
                        values.put(key, Integer.valueOf(encoded));
                    } else if (key != null && "long".equals(tag)) {
                        values.put(key, Long.valueOf(encoded));
                    } else if (key != null && "float".equals(tag)) {
                        values.put(key, Float.valueOf(encoded));
                    } else if (key != null && "boolean".equals(tag)) {
                        values.put(key, Boolean.valueOf(encoded));
                    }
                } else if (event == XmlPullParser.END_TAG && "set".equals(parser.getName())) {
                    if (setKey != null && setValue != null) values.put(setKey, setValue);
                    setKey = null;
                    setValue = null;
                }
            }
        } catch (Exception error) {
            Log.e("DarwinSharedPreferences", "Could not read " + file, error);
            values.clear();
        } finally {
            if (input != null) {
                try {
                    input.close();
                } catch (IOException ignored) {
                }
            }
        }
    }

    private boolean writeToDisk() {
        File parent = file.getParentFile();
        if (parent == null || (!parent.exists() && !parent.mkdirs())) return false;
        if (file.exists()) {
            if (backupFile.exists()) {
                if (!file.delete()) return false;
            } else if (!file.renameTo(backupFile)) {
                return false;
            }
        }
        boolean success = false;
        FileOutputStream output = null;
        try {
            output = new FileOutputStream(file);
            XmlSerializer serializer = resolveSerializer(output);
            serializer.startDocument("utf-8", true);
            serializer.startTag(null, "map");
            for (Map.Entry<String, Object> entry : values.entrySet()) {
                writeValue(serializer, entry.getKey(), entry.getValue());
            }
            serializer.endTag(null, "map");
            serializer.endDocument();
            output.getFD().sync();
            success = true;
        } catch (Exception ignored) {
            success = false;
        } finally {
            if (output != null) {
                try {
                    output.close();
                } catch (IOException ignored) {
                    success = false;
                }
            }
        }
        if (success) {
            backupFile.delete();
        } else {
            file.delete();
            backupFile.renameTo(file);
        }
        return success;
    }

    private static void writeValue(XmlSerializer serializer, String key, Object value)
            throws IOException {
        if (value instanceof String) {
            serializer.startTag(null, "string");
            serializer.attribute(null, "name", key);
            serializer.text((String) value);
            serializer.endTag(null, "string");
        } else if (value instanceof Set) {
            serializer.startTag(null, "set");
            serializer.attribute(null, "name", key);
            for (Object member : (Set<?>) value) {
                serializer.startTag(null, "string");
                serializer.text((String) member);
                serializer.endTag(null, "string");
            }
            serializer.endTag(null, "set");
        } else {
            String tag;
            if (value instanceof Integer) tag = "int";
            else if (value instanceof Long) tag = "long";
            else if (value instanceof Float) tag = "float";
            else if (value instanceof Boolean) tag = "boolean";
            else return;
            serializer.startTag(null, tag);
            serializer.attribute(null, "name", key);
            serializer.attribute(null, "value", String.valueOf(value));
            serializer.endTag(null, tag);
        }
    }

    private static XmlPullParser resolvePullParser(InputStream input) throws Exception {
        Method method = Xml.class.getDeclaredMethod("resolvePullParser", InputStream.class);
        method.setAccessible(true);
        return (XmlPullParser) method.invoke(null, input);
    }

    private static XmlSerializer resolveSerializer(OutputStream output) throws Exception {
        Method method = Xml.class.getDeclaredMethod("resolveSerializer", OutputStream.class);
        method.setAccessible(true);
        return (XmlSerializer) method.invoke(null, output);
    }
}

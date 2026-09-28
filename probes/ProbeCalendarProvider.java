package dev.aim.probe;

import android.content.ContentProvider;
import android.content.ContentUris;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.provider.CalendarContract;
import android.text.TextUtils;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.TimeZone;

/** In-process CalendarContract provider used before a Darwin system_server exists. */
final class ProbeCalendarProvider extends ContentProvider {
    private static final long LOCAL_CALENDAR_ID = 1L;
    private static final int LOCAL_COLOR = 0xff3f51b5;

    private final Map<Long, ContentValues> events = new LinkedHashMap<>();
    private long nextEventId = 1L;

    @Override
    public boolean onCreate() {
        return true;
    }

    @Override
    public synchronized Cursor query(Uri uri, String[] projection, String selection,
            String[] selectionArgs, String sortOrder) {
        String[] columns = projection == null ? new String[] {"_id"} : projection;
        List<String> path = uri.getPathSegments();
        String table = path.isEmpty() ? "" : path.get(0);
        if ("calendars".equals(table)) {
            MatrixCursor cursor = new MatrixCursor(columns, 1);
            addCalendarRow(cursor, columns);
            return cursor;
        }
        if ("events".equals(table)) {
            return queryEvents(uri, columns, selectionArgs);
        }
        if ("instances".equals(table)) {
            return queryInstances(uri, columns);
        }
        if ("event_days".equals(table)) {
            return queryEventDays(uri, columns);
        }
        return new MatrixCursor(columns);
    }

    @Override
    public synchronized Uri insert(Uri uri, ContentValues source) {
        List<String> path = uri.getPathSegments();
        if (path.isEmpty() || !"events".equals(path.get(0))) return null;
        ContentValues event = source == null ? new ContentValues() : new ContentValues(source);
        Long suppliedId = event.getAsLong(CalendarContract.Events._ID);
        long id = suppliedId == null ? nextEventId++ : suppliedId.longValue();
        nextEventId = Math.max(nextEventId, id + 1L);
        event.put(CalendarContract.Events._ID, Long.valueOf(id));
        if (!event.containsKey(CalendarContract.Events.CALENDAR_ID)) {
            event.put(CalendarContract.Events.CALENDAR_ID, Long.valueOf(LOCAL_CALENDAR_ID));
        }
        if (TextUtils.isEmpty(event.getAsString(CalendarContract.Events.EVENT_TIMEZONE))) {
            event.put(CalendarContract.Events.EVENT_TIMEZONE, TimeZone.getDefault().getID());
        }
        if (!event.containsKey(CalendarContract.Events.DTEND)
                && event.containsKey(CalendarContract.Events.DTSTART)) {
            event.put(CalendarContract.Events.DTEND,
                    Long.valueOf(event.getAsLong(CalendarContract.Events.DTSTART) + 3600000L));
        }
        events.put(Long.valueOf(id), event);
        changed(uri);
        return ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, id);
    }

    @Override
    public synchronized int update(Uri uri, ContentValues values, String selection,
            String[] selectionArgs) {
        long id = requestedId(uri, selectionArgs);
        ContentValues event = events.get(Long.valueOf(id));
        if (event == null || values == null) return 0;
        event.putAll(values);
        changed(uri);
        return 1;
    }

    @Override
    public synchronized int delete(Uri uri, String selection, String[] selectionArgs) {
        long id = requestedId(uri, selectionArgs);
        if (id >= 0L && events.remove(Long.valueOf(id)) != null) {
            changed(uri);
            return 1;
        }
        return 0;
    }

    @Override
    public String getType(Uri uri) {
        return "vnd.android.cursor.dir/vnd.aim.calendar";
    }

    private Cursor queryEvents(Uri uri, String[] columns, String[] selectionArgs) {
        MatrixCursor cursor = new MatrixCursor(columns, events.size());
        long requested = requestedId(uri, selectionArgs);
        for (Map.Entry<Long, ContentValues> entry : events.entrySet()) {
            if (requested >= 0L && entry.getKey().longValue() != requested) continue;
            MatrixCursor.RowBuilder row = cursor.newRow();
            for (String column : columns) {
                row.add(eventValue(entry.getValue(), column));
            }
        }
        return cursor;
    }

    private Cursor queryInstances(Uri uri, String[] columns) {
        MatrixCursor cursor = new MatrixCursor(columns, events.size());
        long[] window = trailingRange(uri);
        for (Map.Entry<Long, ContentValues> entry : events.entrySet()) {
            ContentValues event = entry.getValue();
            long begin = longValue(event, CalendarContract.Events.DTSTART, 0L);
            long end = longValue(event, CalendarContract.Events.DTEND, begin + 3600000L);
            if (window != null && (end < window[0] || begin > window[1])) continue;
            MatrixCursor.RowBuilder row = cursor.newRow();
            for (String column : columns) {
                row.add(instanceValue(entry.getKey().longValue(), event, column, begin, end));
            }
        }
        return cursor;
    }

    private Cursor queryEventDays(Uri uri, String[] columns) {
        MatrixCursor cursor = new MatrixCursor(columns, events.size());
        for (ContentValues event : events.values()) {
            long begin = longValue(event, CalendarContract.Events.DTSTART, 0L);
            long end = longValue(event, CalendarContract.Events.DTEND, begin + 3600000L);
            MatrixCursor.RowBuilder row = cursor.newRow();
            for (String column : columns) {
                if (CalendarContract.EventDays.STARTDAY.equals(column)) {
                    row.add(Integer.valueOf(julianDay(begin)));
                } else if (CalendarContract.EventDays.ENDDAY.equals(column)) {
                    row.add(Integer.valueOf(julianDay(end)));
                } else {
                    row.add(Integer.valueOf(0));
                }
            }
        }
        return cursor;
    }

    private static void addCalendarRow(MatrixCursor cursor, String[] columns) {
        MatrixCursor.RowBuilder row = cursor.newRow();
        for (String column : columns) row.add(calendarValue(column));
    }

    static Object calendarValue(String column) {
        if (CalendarContract.Calendars._ID.equals(column)) return Long.valueOf(LOCAL_CALENDAR_ID);
        if (CalendarContract.Calendars.CALENDAR_DISPLAY_NAME.equals(column)) {
            return "Darwin Calendar";
        }
        if (CalendarContract.Calendars.OWNER_ACCOUNT.equals(column)
                || CalendarContract.Calendars.ACCOUNT_NAME.equals(column)) {
            return "local@darwin";
        }
        if (CalendarContract.Calendars.ACCOUNT_TYPE.equals(column)) {
            return CalendarContract.ACCOUNT_TYPE_LOCAL;
        }
        if (CalendarContract.Calendars.CALENDAR_COLOR.equals(column)) {
            return Integer.valueOf(LOCAL_COLOR);
        }
        if (CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL.equals(column)) {
            return Integer.valueOf(CalendarContract.Calendars.CAL_ACCESS_OWNER);
        }
        if (CalendarContract.Calendars.VISIBLE.equals(column)) return Integer.valueOf(1);
        if (CalendarContract.Calendars.MAX_REMINDERS.equals(column)) return Integer.valueOf(5);
        if (CalendarContract.Calendars.ALLOWED_REMINDERS.equals(column)) return "0,1";
        if (CalendarContract.Calendars.ALLOWED_ATTENDEE_TYPES.equals(column)) return "0,1,2";
        if (CalendarContract.Calendars.ALLOWED_AVAILABILITY.equals(column)) return "0,1,2";
        if (CalendarContract.Calendars.CAN_ORGANIZER_RESPOND.equals(column)) {
            return Integer.valueOf(0);
        }
        return Integer.valueOf(0);
    }

    private static Object eventValue(ContentValues event, String column) {
        Object value = event.get(column);
        if (value != null) return value;
        if (CalendarContract.Events.CALENDAR_COLOR.equals(column)) {
            return Integer.valueOf(LOCAL_COLOR);
        }
        if (CalendarContract.Events.OWNER_ACCOUNT.equals(column)
                || CalendarContract.Events.ORGANIZER.equals(column)) {
            return "local@darwin";
        }
        if (CalendarContract.Events.EVENT_TIMEZONE.equals(column)) {
            return TimeZone.getDefault().getID();
        }
        return Integer.valueOf(0);
    }

    private static Object instanceValue(long id, ContentValues event, String column,
            long begin, long end) {
        if (CalendarContract.Instances.EVENT_ID.equals(column)) return Long.valueOf(id);
        if (CalendarContract.Instances._ID.equals(column)) return Long.valueOf(id);
        if (CalendarContract.Instances.BEGIN.equals(column)) return Long.valueOf(begin);
        if (CalendarContract.Instances.END.equals(column)) return Long.valueOf(end);
        if (CalendarContract.Instances.START_DAY.equals(column)) {
            return Integer.valueOf(julianDay(begin));
        }
        if (CalendarContract.Instances.END_DAY.equals(column)) {
            return Integer.valueOf(julianDay(end));
        }
        if (CalendarContract.Instances.START_MINUTE.equals(column)) {
            return Integer.valueOf(minuteOfDay(begin));
        }
        if (CalendarContract.Instances.END_MINUTE.equals(column)) {
            return Integer.valueOf(minuteOfDay(end));
        }
        if (CalendarContract.Instances.DISPLAY_COLOR.equals(column)
                || CalendarContract.Instances.CALENDAR_COLOR.equals(column)) {
            return Integer.valueOf(LOCAL_COLOR);
        }
        if (column.contains(" AS dispAllday")) {
            return event.getAsBoolean(CalendarContract.Events.ALL_DAY) == Boolean.TRUE
                    || end - begin >= 86400000L ? Integer.valueOf(1) : Integer.valueOf(0);
        }
        return eventValue(event, column);
    }

    private static int julianDay(long millis) {
        return (int) Math.floorDiv(millis, 86400000L) + 2440588;
    }

    private static int minuteOfDay(long millis) {
        TimeZone zone = TimeZone.getDefault();
        return (int) Math.floorMod(millis + zone.getOffset(millis), 86400000L) / 60000;
    }

    private static long longValue(ContentValues values, String key, long defaultValue) {
        Long value = values.getAsLong(key);
        return value == null ? defaultValue : value.longValue();
    }

    private static long requestedId(Uri uri, String[] selectionArgs) {
        List<String> path = uri.getPathSegments();
        if (path.size() > 1) {
            try {
                return Long.parseLong(path.get(path.size() - 1));
            } catch (NumberFormatException ignored) {}
        }
        if (selectionArgs != null && selectionArgs.length == 1) {
            try {
                return Long.parseLong(selectionArgs[0]);
            } catch (NumberFormatException ignored) {}
        }
        return -1L;
    }

    private static long[] trailingRange(Uri uri) {
        List<String> path = uri.getPathSegments();
        if (path.size() < 3) return null;
        try {
            return new long[] {
                    Long.parseLong(path.get(path.size() - 2)),
                    Long.parseLong(path.get(path.size() - 1))};
        } catch (NumberFormatException ignored) {
            return null;
        }
    }

    private void changed(Uri uri) {
        if (getContext() != null) getContext().getContentResolver().notifyChange(uri, null);
    }
}

package dev.darwinart.runtime.am;

import android.os.IBinder;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Map;

/**
 * The ActiveServices-owned connection ledger.
 *
 * Every method is package-private and expects the caller to hold the single
 * ActiveServices monitor.  This class has no monitor of its own and performs
 * no Binder calls; death-recipient link/unlink remains a resource operation
 * performed by the policy owner after a returned detached batch is complete.
 */
final class ServiceConnectionIndex {
    static final class DeathPin {
        final IBinder binder;
        final IBinder.DeathRecipient recipient;

        DeathPin(IBinder connectionBinder, IBinder.DeathRecipient deathRecipient) {
            binder = connectionBinder;
            recipient = deathRecipient;
        }
    }

    static final class Detached {
        private final ArrayList<ConnectionRecord> records = new ArrayList<>();
        private final ArrayList<DeathPin> deaths = new ArrayList<>();

        List<ConnectionRecord> records() {
            return Collections.unmodifiableList(records);
        }

        List<DeathPin> deaths() {
            return Collections.unmodifiableList(deaths);
        }

        boolean isEmpty() {
            return records.isEmpty() && deaths.isEmpty();
        }

        private void addRecord(ConnectionRecord record) {
            records.add(record);
        }

        private void addDeath(IBinder binder, IBinder.DeathRecipient recipient) {
            deaths.add(new DeathPin(binder, recipient));
        }
    }

    private final HashMap<IBinder, ArrayList<ConnectionRecord>> recordsByBinder = new HashMap<>();
    private final HashMap<IBinder, IBinder.DeathRecipient> deathsByBinder = new HashMap<>();

    /** Returns a new snapshot; mutations to it do not affect the ledger. */
    ArrayList<ConnectionRecord> records(IBinder binder) {
        ArrayList<ConnectionRecord> records = recordsByBinder.get(binder);
        return records == null ? new ArrayList<>() : new ArrayList<>(records);
    }

    IBinder.DeathRecipient death(IBinder binder) {
        return deathsByBinder.get(binder);
    }

    void setDeath(IBinder binder, IBinder.DeathRecipient recipient) {
        if (binder == null) throw new IllegalArgumentException("Missing connection Binder");
        if (recipient == null) deathsByBinder.remove(binder);
        else deathsByBinder.put(binder, recipient);
    }

    void add(ConnectionRecord record) {
        if (record == null || record.connectionBinder == null || record.binding == null) {
            throw new IllegalArgumentException("Invalid service connection record");
        }
        if (contains(record)) return;
        recordsByBinder.computeIfAbsent(record.connectionBinder, unused -> new ArrayList<>())
                .add(record);
        record.binding.connections.add(record);
    }

    boolean contains(ConnectionRecord record) {
        if (record == null) return false;
        ArrayList<ConnectionRecord> records = recordsByBinder.get(record.connectionBinder);
        return records != null && records.contains(record);
    }

    /** Detaches one exact record and any death pin made unused by that removal. */
    Detached remove(ConnectionRecord record) {
        Detached detached = new Detached();
        if (record == null) return detached;
        ArrayList<ConnectionRecord> records = recordsByBinder.get(record.connectionBinder);
        if (records == null || !records.remove(record)) return detached;
        record.binding.connections.remove(record);
        detached.addRecord(record);
        finishBinderRemoval(record.connectionBinder, records, detached);
        return detached;
    }

    /** Detaches every record for one client Binder. */
    Detached detachBinder(IBinder binder) {
        Detached detached = new Detached();
        if (binder == null) return detached;
        ArrayList<ConnectionRecord> records = recordsByBinder.remove(binder);
        if (records == null) return detached;
        for (ConnectionRecord record : records) {
            record.binding.connections.remove(record);
            detached.addRecord(record);
        }
        takeDeath(binder, detached);
        return detached;
    }

    /** Detaches records owned by one exact application process incarnation. */
    Detached detachClient(ApplicationProcessRegistry.AttachedApplication gone) {
        Detached detached = new Detached();
        if (gone == null) return detached;
        Iterator<Map.Entry<IBinder, ArrayList<ConnectionRecord>>> iterator =
                recordsByBinder.entrySet().iterator();
        while (iterator.hasNext()) {
            Map.Entry<IBinder, ArrayList<ConnectionRecord>> entry = iterator.next();
            ArrayList<ConnectionRecord> records = entry.getValue();
            for (Iterator<ConnectionRecord> recordsIterator = records.iterator();
                    recordsIterator.hasNext();) {
                ConnectionRecord record = recordsIterator.next();
                if (!record.owner.matches(gone)) continue;
                recordsIterator.remove();
                record.binding.connections.remove(record);
                detached.addRecord(record);
            }
            if (records.isEmpty()) {
                IBinder binder = entry.getKey();
                iterator.remove();
                takeDeath(binder, detached);
            }
        }
        return detached;
    }

    /** Detaches all records belonging to one Intent binding. */
    Detached detachBinding(IntentBindRecord binding) {
        Detached detached = new Detached();
        if (binding == null) return detached;
        Iterator<Map.Entry<IBinder, ArrayList<ConnectionRecord>>> iterator =
                recordsByBinder.entrySet().iterator();
        while (iterator.hasNext()) {
            Map.Entry<IBinder, ArrayList<ConnectionRecord>> entry = iterator.next();
            ArrayList<ConnectionRecord> records = entry.getValue();
            for (Iterator<ConnectionRecord> recordsIterator = records.iterator();
                    recordsIterator.hasNext();) {
                ConnectionRecord record = recordsIterator.next();
                if (record.binding != binding) continue;
                recordsIterator.remove();
                binding.connections.remove(record);
                detached.addRecord(record);
            }
            if (records.isEmpty()) {
                IBinder binder = entry.getKey();
                iterator.remove();
                takeDeath(binder, detached);
            }
        }
        return detached;
    }

    private void finishBinderRemoval(
            IBinder binder, ArrayList<ConnectionRecord> records, Detached detached) {
        if (!records.isEmpty()) return;
        recordsByBinder.remove(binder);
        takeDeath(binder, detached);
    }

    private void takeDeath(IBinder binder, Detached detached) {
        IBinder.DeathRecipient recipient = deathsByBinder.remove(binder);
        if (recipient != null) detached.addDeath(binder, recipient);
    }
}

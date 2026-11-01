"""Compile exact android-16.0.0_r1 reader bodies with desktop boundary adapters.
AOSP bodies are Apache-2.0, Copyright The Android Open Source Project.
Sources must match sources.lock; a hash-verified local cache supports sandbox runs.
"""
import hashlib
from pathlib import Path
import re
import subprocess
import sys

repo, work, jdk = map(Path, sys.argv[1:4])
fixture = Path(__file__).parent
lock = (repo / 'crates/aim-services/sources.lock').read_text()

def source(name):
    project, path, digest = re.search(r'(platform/frameworks/[^|]+)\|([^"|]*/' + name + r'\.(?:java|proto))\|([a-f0-9]+)', lock).groups()
    file = repo / '_build/aosp' / project.removeprefix('platform/') / path
    if not file.is_file():
        file = repo / 'target/aim-cache/battery-usage-sources' / Path(path).name
    data = file.read_bytes()  # Missing inputs fail; never silently skip the oracle.
    assert hashlib.sha256(data).hexdigest() == digest, name
    text = re.sub(r'/\*.*?\*/|//[^\n]*', '', data.decode(), flags=re.S)
    return re.sub(r'@[\w.]+(?:\([^)]*\))?\s*', '', text)

def block(text, signature):
    start = text.index(signature)
    opening = text.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (text[end] == '{') - (text[end] == '}')
        end += 1
    return text[start:end]

def constants(text):
    return '\n'.join(re.findall(r'(?:public |private |protected )?static final int \w+\s*=[^;]+;', text))

def methods(text, signatures):
    return '\n'.join(block(text, sig) for sig in signatures)

proto = source('enums')
proto_class = 'static class OsProtoEnums {' + ''.join('static final int ' + n + '=' + v + ';' for n, v in re.findall(r'(POWER_COMPONENT_\w+) = (\d+);', proto)) + '}\n'
bc = source('BatteryConsumer')
bc_code = constants(bc[:bc.index('public static final class Dimensions')])
bc_code += '\n' + block(bc, 'static {\n        int[] supportedPowerComponents')
bc_code += '\nprivate static final IntArray SUPPORTED_POWER_COMPONENTS_PER_PROCESS_STATE;\n'
bc_code += 'protected final BatteryConsumerData mData; protected final PowerComponents mPowerComponents;\n'
bc_code += methods(bc, ['public BatteryConsumer(BatteryConsumerData data)',
    'public static final class Key', 'static class BatteryConsumerData {',
    'static class BatteryConsumerDataLayout', 'static BatteryConsumerDataLayout createBatteryConsumerDataLayout',
    'public Key getKey(int componentId, int processState)',
    'public long getUsageDurationMillis(int componentId)',
    'public long getUsageDurationMillis(Key key)', 'public double getConsumedPower(int componentId)',
    'public int getPowerModel(int componentId)'])
bc_code += '\nstatic final String TAG = \"consumer\"; static String powerComponentIdToString(int i) {return "component";}\n'
bc_code += 'static String processStateToString(int i) {return "state";}\n'
bc_code += 'static String[] sScreenStateNames = new String[3], sPowerStateNames = new String[3];\n'
classes = proto_class + constants(bc[:bc.index('public static final class Dimensions')]) + '\nstatic class BatteryConsumer {\n' + bc_code + '\n}\n'
for name, signatures in [
    ('UidBatteryConsumer', ['UidBatteryConsumer(BatteryConsumerData data)', 'public int getUid()',
                           'public long getTimeInProcessStateMs(int state)']),
    ('AggregateBatteryConsumer', ['AggregateBatteryConsumer(BatteryConsumerData data)', 'int getScope()']),
    ('UserBatteryConsumer', ['UserBatteryConsumer(BatteryConsumerData data)'])]:
    text = source(name)
    classes += 'static class ' + name + ' extends BatteryConsumer {\n' + constants(text) + '\n'
    classes += methods(text, signatures) + '\n}\n'
pc = source('PowerComponents')
classes += 'static class PowerComponents { final BatteryConsumer.BatteryConsumerData mData;\n'
classes += methods(pc, ['PowerComponents(BatteryConsumer.BatteryConsumerData data)',
    'public double getConsumedPower(int powerComponent,',
    'public long getUsageDurationMillis(int powerComponent,',
    'public long getUsageDurationMillis(BatteryConsumer.Key key)']) + '\n}\n'
bus = source('BatteryUsageStats')
fields = bus[bus.index('private final int mDischargePercentage;'):bus.index('private BatteryUsageStats(Builder')]
classes += 'static class BatteryUsageStats {\n' + constants(bus[:bus.index('private final int')]) + '\n' + fields
classes += methods(bus, ['private BatteryUsageStats(Parcel source)', 'public void writeToParcel(Parcel dest, int flags)'])
classes += '\n}\n'
history = source('BatteryStatsHistory')
# Constructor and fragment reader are original. Unused producer-only fields are
# boundary doubles, since the native owner supplies the history blobs.
classes += '''static class BatteryStatsHistory {
    Object mClock, mTracer, mStore, mEventLogger, mWritableHistory, mMonotonicClock;
    boolean mMutable; Parcel mHistoryBuffer; ArrayList<Parcel> mHistoryParcels;
    static final boolean DEBUG = false; static final String TAG = "history";
'''
classes += methods(history, ['private BatteryStatsHistory(Parcel parcel)',
    'private void readFromParcel(Parcel in, boolean useBlobs)',
    'public static BatteryStatsHistory createFromBatteryUsageStatsParcel(Parcel in)'])
classes += '''
    void writeToBatteryUsageStatsParcel(Parcel out, long preferred) {
        out.writeBlob(mHistoryBuffer.marshall());
        for (Parcel p : mHistoryParcels) { out.writeBoolean(true); out.writeBlob(p.marshall()); }
        out.writeBoolean(false);
    }
}\n'''
java = (fixture / 'Oracle.java.in').read_text().replace('@ORIGINAL@', classes)
(work / 'Oracle.java').write_text(java)
subprocess.run([str(jdk / 'javac'), '-d', str(work), str(work / 'Oracle.java')], check=True)
subprocess.run([str(jdk / 'java'), '-cp', str(work), 'Oracle', str(work)], check=True)

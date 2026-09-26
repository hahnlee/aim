#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
apkanalyzer="${APKANALYZER:-$HOME/Library/Android/sdk/cmdline-tools/latest/bin/apkanalyzer}"
java_home="${JAVA_HOME:-/opt/homebrew/opt/openjdk@17}"
out="$root/_build/job-scheduler-endpoint-test"

source "$root/upstream/android16-job-scheduler.lock"
framework="$root/$FRAMEWORK_JOB_SCHEDULER_PATH"
test "$(shasum -a 256 "$framework" | awk '{print $1}')" = "$FRAMEWORK_JOB_SCHEDULER_SHA256"
test -x "$apkanalyzer"
grep -Fq "services.put(\"$JOB_SCHEDULER_SERVICE_NAME\", new JobSchedulerEndpoint(processes, jobs));" \
  "$root/runtime/framework/system/SystemServiceFactory.java"

packages="$out/framework-packages.txt"
mkdir -p "$out"
"$apkanalyzer" dex packages --defined-only "$framework" > "$packages"
grep -Fq "$IJOB_SCHEDULER_DESCRIPTOR $SCHEDULE_SIGNATURE" "$packages"
grep -Fq "$IJOB_SCHEDULER_DESCRIPTOR $SCHEDULE_AS_PACKAGE_SIGNATURE" "$packages"

transaction_names="$out/transaction-names.smali"
"$apkanalyzer" dex code \
  --class 'android.app.job.IJobScheduler$Stub' \
  --method 'getDefaultTransactionName(I)Ljava/lang/String;' "$framework" > "$transaction_names"
transaction_for() {
  local method="$1" target
  target="$(awk -v wanted="\"$method\"" '
    /^[[:space:]]*:pswitch_/ { label=$1 }
    ($1 == "const-string" || $1 == "const-string/jumbo") && $3 == wanted { print label; exit }
  ' "$transaction_names")"
  awk -v target="$target" '
    /^[[:space:]]*\.packed-switch 0x1/ { inside=1; number=1; next }
    inside && /^[[:space:]]*:pswitch_/ { if ($1 == target) { print number; exit } number++ }
  ' "$transaction_names"
}
test "$(transaction_for schedule)" = "$SCHEDULE_TRANSACTION"
test "$(transaction_for scheduleAsPackage)" = "$SCHEDULE_AS_PACKAGE_TRANSACTION"
test "$(transaction_for getAllPendingJobs)" = "$GET_ALL_PENDING_JOBS_TRANSACTION"
test "$(transaction_for notePendingUserRequestedAppStop)" = "$NOTE_PENDING_USER_REQUESTED_APP_STOP_TRANSACTION"

rm -rf "$out/classes"
mkdir -p "$out/classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$out/classes" \
  "$root/tools/tests/jobscheduler/stubs/android/app/job/JobWorkItem.java" \
  "$root/tools/tests/jobscheduler/stubs/android/app/job/JobInfo.java" \
  "$root/tools/tests/jobscheduler/stubs/android/content/pm/ParceledListSlice.java" \
  "$root/tools/tests/jobscheduler/stubs/android/content/ComponentName.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/IBinder.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/Parcel.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/Parcelable.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/Binder.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/RemoteException.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/Bundle.java" \
  "$root/tools/tests/jobscheduler/stubs/android/os/PersistableBundle.java" \
  "$root/tools/tests/jobscheduler/stubs/android/content/ClipData.java" \
  "$root/tools/tests/jobscheduler/stubs/dev/darwinart/runtime/job/JobSchedulerService.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/job/JobSchedulerEndpoint.java" \
  "$root/tools/tests/jobscheduler/JobSchedulerEndpointTest.java"
"$java_home/bin/java" -ea -cp "$out/classes" dev.darwinart.runtime.job.JobSchedulerEndpointTest

rm -rf "$out/service-classes"
mkdir -p "$out/service-classes"
"$java_home/bin/javac" --release 8 -encoding UTF-8 -d "$out/service-classes" \
  $(find "$root/tools/tests/jobscheduler/stubs" -name '*.java' \
      ! -path '*/stubs/dev/darwinart/runtime/job/JobSchedulerService.java' -print) \
  "$root/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$root/runtime/framework/am/SystemServiceBindings.java" \
  "$root/tools/tests/connectivity/stubs/android/net/ProxyInfo.java" \
  "$root/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$root/runtime/framework/connectivity/ConnectivityState.java" \
  "$root/runtime/framework/connectivity/ConnectivitySnapshot.java" \
  "$root/runtime/framework/am/PackageQueries.java" \
  "$root/runtime/framework/job/JobRecord.java" \
  "$root/runtime/framework/job/JobServiceContext.java" \
  "$root/runtime/framework/job/JobSchedulerService.java" \
  "$root/tools/tests/jobscheduler/JobSchedulerServiceTest.java"
"$java_home/bin/java" -ea -cp "$out/service-classes" \
  dev.darwinart.runtime.job.JobSchedulerServiceTest

# System-process entry boundary

Run `bash tools/tests/system-process-entry/run.sh` from the repository root.
The test compiles the production native system entry into an isolated JNI dylib
and runs it with OpenJDK. No Probe classes exist on the test classpath.

Test-only substitutes cover Android service classes and native framework,
compositor and Binder dependencies, and for the AOSP bootstrap sequence
(RuntimeInit, standalone system-server class loaders, SystemServerBootstrap,
the host command relay). Checks cover endpoint arguments/return propagation, missing socket rejection, preserved Java
initialization exceptions and compositor failure preventing Binder publication.
Internal kernel Binder setup precedes system Context creation; Context and
compatibility-policy initialization precede external Binder readiness;
missing Context and catalog exceptions prevent publication. Those dependency
implementations are test substitutes, not original framework initialization proof.
The production classpath builder is also checked for the image's
SYSTEMSERVERCLASSPATH (its `system/etc/classpath` export), the support DEX,
spaces in paths, and rejection of relative/parent/list injection
without changing the prior result on failure.
These files are never runtime build inputs. This is not an ART/framework or
live application acceptance test. The temporary directory is removed on exit.

# System-process entry boundary

Run `bash tools/tests/system-process-entry/run.sh` from the repository root.
The test compiles the production native system entry into an isolated JNI dylib
and runs it with OpenJDK. No Probe classes exist on the test classpath.

Test-only substitutes cover Android service classes and native framework,
compositor and Binder dependencies. Checks cover explicit resolver registration,
endpoint arguments/return propagation, missing socket rejection, preserved Java
initialization exceptions and compositor failure preventing Binder publication.
The production classpath builder is also checked for the image service JAR,
support DEX, spaces in paths, and rejection of relative/parent/list injection
without changing the prior result on failure.
These files are never runtime build inputs. This is not an ART/framework or
live application acceptance test. The temporary directory is removed on exit.

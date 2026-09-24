# Darwin Android x18 task ABI

Android arm64 ELF code can use x18 as a general register. A current-SDK macOS
task cannot: Darwin reserves x18 and XNU clears it when scheduling a thread
unless the task opts into the custom-x18 ABI. XNU also provides a compatibility
contract for Mach-O tasks whose declared SDK predates macOS 13.

`declare-darwin-x18-abi.sh` applies that task ABI declaration to the AIM
host before it is signed. It does not modify an APK or any guest DSO. Every
thread in the host process then preserves x18, covering guest-internal code as
well as JNI and provider calls.

`prepare-darwin-art-host.sh` is the launch-time owner of this contract. Cargo
can replace the host after an APK was installed, so every launch verifies the
SDK declaration and development entitlement under a file lock. It repairs and
signs only a stale development host; packaged runtimes fail closed instead of
mutating shipped code.

Run `tools/darwin-x18-abi/audit.sh`. Its arm64 assembly fixture keeps a sentinel
in x18 across 100,000 forced scheduling points. A normal current-SDK executable
must lose the sentinel; both the directly declared executable and two racing
installed-launch preparations must preserve it.

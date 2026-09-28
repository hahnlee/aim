# Binder endpoint ownership

`RemoteBinder` is the Java endpoint for the Darwin Binder transport. It borrows
a channel and target ID; it does not spawn, stop or own an app/service process.
Launcher process leases remain with their caller. No Android lifecycle policy
belongs in this class. AIDL callers see a remote interface (no local owner).

`compat/binder/remote_binder_jni.cc` registers the transport independently of
ProbeContext. Both app-created endpoints and imported Binder references register
this API on the relevant runtime class, allowing system-side callbacks without
an Activity bootstrap. The existing wire implementation owns Parcel/Binder/FD
transfer and channel dispatch; this module does not implement another protocol.

This does not implement ActivityManagerService, attach/bind publication, Binder
death notifications or generation-safe Java channel handles. Those contracts
remain separate work. Moving this endpoint must not be reported as successful
application binding. Runtime DEX gates require this class; the old ProbeContext
inner endpoint is no longer packaged.

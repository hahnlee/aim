package dev.darwinart.runtime.wm;

import android.os.Binder;
import android.os.IBinder;
import android.os.Parcel;
import android.view.IWindowManager;
import android.view.IWindowSession;
import android.view.InputChannel;
import android.view.View;
import android.view.WindowManager;
import android.os.HandlerThread;
import dev.darwinart.runtime.am.ApplicationProcessRegistry;

/**
 * Exercises the real WMS endpoint, identity, ownership, and surface registry
 * with only transport/platform boundaries replaced by test-local stubs.
 */
public final class WindowManagerEndpointFixtureTest {
    private static final String WMS = "android.view.IWindowManager";
    private static final String SESSION = "android.view.IWindowSession";
    private static int checks;

    private static void check(boolean value, String message) {
        ++checks;
        if (!value) throw new AssertionError(message);
    }

    private static void expectFailure(Runnable action, String message) {
        ++checks;
        try {
            action.run();
            throw new AssertionError("expected failure: " + message);
        } catch (RuntimeException expected) {
            // Authentication and exact-lifecycle violations are both fail-closed.
        }
    }

    private static final class Fixture {
        final ApplicationProcessRegistry processes = new ApplicationProcessRegistry();
        final DesktopWindowMetadataRegistry metadata = new DesktopWindowMetadataRegistry();
        final WindowManagerEndpoint manager =
                new WindowManagerEndpoint(processes, metadata);

        WindowSessionEndpoint open(int pid, int uid, IBinder callback) throws Exception {
            Binder.setCallingIdentity(pid, uid);
            Parcel data = Parcel.obtain();
            data.writeInterfaceToken(WMS);
            data.writeStrongBinder(callback);
            Parcel reply = Parcel.obtain();
            check(manager.onTransact(IWindowManager.Stub.TRANSACTION_openSession,
                    data, reply, 0), "openSession handled");
            reply.readException();
            return (WindowSessionEndpoint) reply.readStrongBinder();
        }

        void identify(int pid, int uid, IBinder thread, long sequence) {
            processes.beginAttachment(pid, uid, thread, sequence);
            processes.identify(pid, sequence, thread, "fixture.pkg." + pid);
        }
    }

    private static WindowManager.LayoutParams attrs() {
        WindowManager.LayoutParams attrs = new WindowManager.LayoutParams();
        attrs.type = WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW;
        attrs.width = 320;
        attrs.height = 240;
        attrs.accessibilityTitle = "fixture";
        return attrs;
    }

    private static Parcel addData(IBinder window, WindowManager.LayoutParams attrs) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(SESSION);
        data.writeStrongBinder(window);
        data.writeTypedObject(attrs, 0);
        data.writeInt(View.VISIBLE);
        data.writeInt(0); // layer stack
        data.writeInt(0); // user id
        data.writeInt(0); // requested visible inset types
        data.writeInt(1); // one scale element
        return data;
    }

    private static Parcel relayoutData(IBinder window) {
        return relayoutData(window, attrs());
    }

    private static Parcel relayoutData(IBinder window, WindowManager.LayoutParams layout) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(SESSION);
        data.writeStrongBinder(window);
        data.writeTypedObject(layout, 0);
        data.writeInt(320); // requested width
        data.writeInt(240); // requested height
        data.writeInt(View.VISIBLE);
        data.writeInt(0); // flags
        data.writeInt(1); // sequence
        data.writeInt(0); // last sync sequence
        return data;
    }

    private static Parcel removeData(IBinder window) {
        Parcel data = Parcel.obtain();
        data.writeInterfaceToken(SESSION);
        data.writeStrongBinder(window);
        return data;
    }

    private static boolean transact(WindowSessionEndpoint session, int code, Parcel data,
            Parcel reply) throws Exception {
        return session.onTransact(code, data, reply, 0);
    }

    private static void add(WindowSessionEndpoint session, IBinder window) throws Exception {
        check(transact(session, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                addData(window, attrs()), Parcel.obtain()), "add handled");
        pump();
    }

    private static void pump() {
        for (int i = 0; i < 64 && HandlerThread.runLatestNext(); ++i) {}
    }

    private static Fixture base() {
        DesktopWindowMetadataRegistry.reset();
        WindowInputPublisher.reset();
        InputChannel.resetStats();
        Fixture fixture = new Fixture();
        return fixture;
    }

    private static void openAndAuthentication() throws Exception {
        Fixture fixture = base();
        Binder thread = new Binder();
        // The unfinished but identified attachment is a legitimate WMS caller.
        fixture.identify(4101, 10101, thread, 1);
        WindowSessionEndpoint first = fixture.open(4101, 10101, thread);
        WindowSessionEndpoint second = fixture.open(4101, 10101, thread);
        check(first != second, "each openSession creates an independent endpoint");

        int opens = InputChannel.openedCount();
        Binder.setCallingIdentity(4101, 10102);
        expectFailure(() -> {
            try { fixture.open(4101, 10102, thread); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "wrong uid rejected before endpoint IO");
        check(InputChannel.openedCount() == opens, "wrong uid did not open a channel");
        expectFailure(() -> {
            try { transact(first, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(new Binder(), attrs()), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "existing session rejects wrong uid before window IO");
        check(InputChannel.openedCount() == opens, "wrong session uid did not open a channel");
        expectFailure(() -> {
            try { fixture.open(4199, 10101, thread); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "wrong pid rejected before endpoint IO");
        check(InputChannel.openedCount() == opens, "wrong pid did not open a channel");
        Binder.setCallingIdentity(4199, 10101);
        expectFailure(() -> {
            try { transact(first, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(new Binder(), attrs()), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "existing session rejects wrong pid before window IO");
        check(InputChannel.openedCount() == opens, "wrong session pid did not open a channel");

        Binder.setCallingIdentity(4101, 10101);
        fixture.processes.retireAttached(4101, 1, thread);
        fixture.identify(4101, 10101, new Binder(), 2);
        IBinder oldWindow = new Binder();
        expectFailure(() -> {
            try { transact(first, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(oldWindow, attrs()), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "old pid incarnation rejected before channel IO");
        check(InputChannel.openedCount() == opens, "old incarnation did not open a channel");
    }

    private static void duplicateAndUnknownOperations() throws Exception {
        Fixture fixture = base();
        IBinder thread1 = new Binder();
        IBinder thread2 = new Binder();
        fixture.identify(4101, 10101, thread1, 1);
        fixture.identify(4102, 10102, thread2, 2);
        WindowSessionEndpoint one = fixture.open(4101, 10101, thread1);
        WindowSessionEndpoint two = fixture.open(4102, 10102, thread2);
        IBinder window = new Binder();
        Binder.setCallingIdentity(4101, 10101);
        add(one, window);
        int opened = InputChannel.openedCount();
        int disposed = InputChannel.disposedCount();
        Binder.setCallingIdentity(4102, 10102);
        expectFailure(() -> {
            try { transact(two, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(window, attrs()), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "cross-session duplicate window rejected");
        check(InputChannel.openedCount() == opened && InputChannel.disposedCount() == disposed,
                "duplicate rejected before old channel disposal or new IO");

        IBinder unknown = new Binder();
        Binder.setCallingIdentity(4101, 10101);
        int updates = DesktopWindowMetadataRegistry.updateCount;
        int removals = DesktopWindowMetadataRegistry.removeCount;
        int publications = WindowInputPublisher.publishCount + WindowInputPublisher.removeCount;
        expectFailure(() -> {
            try { transact(one, IWindowSession.Stub.TRANSACTION_relayout,
                    relayoutData(unknown), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "unknown relayout rejected");
        expectFailure(() -> {
            try { transact(one, IWindowSession.Stub.TRANSACTION_remove,
                    removeData(unknown), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "unknown remove rejected");
        check(DesktopWindowMetadataRegistry.updateCount == updates,
                "unknown operations have no surface or metadata effects");
        check(DesktopWindowMetadataRegistry.removeCount == removals
                && WindowInputPublisher.publishCount + WindowInputPublisher.removeCount == publications
                && InputChannel.openedCount() == opened && InputChannel.disposedCount() == disposed,
                "unknown operations do not publish, dispose, or remove metadata");

        WindowManager.LayoutParams wrongType = attrs();
        wrongType.type = WindowManager.LayoutParams.FIRST_APPLICATION_WINDOW + 1;
        updates = DesktopWindowMetadataRegistry.updateCount;
        expectFailure(() -> {
            try { transact(one, IWindowSession.Stub.TRANSACTION_relayout,
                    relayoutData(window, wrongType), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "invalid relayout type rejected before surface IO");
        check(DesktopWindowMetadataRegistry.updateCount == updates,
                "invalid relayout does not poison surface attributes");
        check(transact(one, IWindowSession.Stub.TRANSACTION_relayout,
                relayoutData(window, attrs()), Parcel.obtain()),
                "valid relayout remains usable after invalid request");
        pump();
    }

    private static void addFailureCleansForRetry() throws Exception {
        Fixture fixture = base();
        IBinder thread = new Binder();
        fixture.identify(4101, 10101, thread, 1);
        WindowSessionEndpoint session = fixture.open(4101, 10101, thread);
        Binder.setCallingIdentity(4101, 10101);

        IBinder metadataWindow = new Binder();
        DesktopWindowMetadataRegistry.failUpdate = true;
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(metadataWindow, attrs()), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "metadata add failure");
        check(InputChannel.disposedCount() == 2, "metadata failure disposes both channel ends");
        DesktopWindowMetadataRegistry.failUpdate = false;
        add(session, metadataWindow);

        IBinder replyWindow = new Binder();
        Parcel reply = Parcel.obtain();
        reply.failNextWrite();
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(replyWindow, attrs()), reply); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "reply build failure");
        check(InputChannel.disposedCount() == 3,
                "reply failure disposes client while server lease is retained");
        pump();
        check(InputChannel.disposedCount() == 4,
                "reply failure eventually releases the retained server lease");
        add(session, replyWindow);

        IBinder unresolvedWindow = new Binder();
        DesktopWindowMetadataRegistry.failUpdate = true;
        DesktopWindowMetadataRegistry.failRemove = true;
        RuntimeException failure = null;
        try {
            transact(session, IWindowSession.Stub.TRANSACTION_addToDisplayAsUser,
                    addData(unresolvedWindow, attrs()), Parcel.obtain());
        } catch (RuntimeException error) {
            failure = error;
        }
        check(failure != null && failure.getSuppressed().length == 1,
                "failed add reports unresolved cleanup without losing the original error");
        DesktopWindowMetadataRegistry.failUpdate = false;
        DesktopWindowMetadataRegistry.failRemove = false;
        int opens = InputChannel.openedCount();
        expectFailure(() -> {
            try { add(session, unresolvedWindow); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "unresolved add keeps its original token reserved");
        check(InputChannel.openedCount() == opens, "unresolved token cannot open a successor channel");
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_relayout,
                    relayoutData(unresolvedWindow), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "failed add cleanup is not a live window");
        check(transact(session, IWindowSession.Stub.TRANSACTION_remove,
                removeData(unresolvedWindow), Parcel.obtain()), "exact failed-add cleanup retry succeeds");
        add(session, unresolvedWindow);
    }

    private static void removeFailureRetainsRetiringToken() throws Exception {
        Fixture fixture = base();
        IBinder thread = new Binder();
        fixture.identify(4101, 10101, thread, 1);
        WindowSessionEndpoint session = fixture.open(4101, 10101, thread);
        Binder.setCallingIdentity(4101, 10101);
        IBinder publisherWindow = new Binder();
        add(session, publisherWindow);
        int disposed = InputChannel.disposedCount();
        check(transact(session, IWindowSession.Stub.TRANSACTION_remove,
                removeData(publisherWindow), Parcel.obtain()), "remove queues retirement");
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_relayout,
                    relayoutData(publisherWindow), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "retiring registration rejects ordinary relayout");
        check(InputChannel.disposedCount() == disposed,
                "remove retains server channel until delayed TX retirement");
        pump();
        check(InputChannel.disposedCount() == disposed + 1,
                "delayed driver turn releases removed server channel");

        IBinder cleanupWindow = new Binder();
        add(session, cleanupWindow);
        DesktopWindowMetadataRegistry.failRemove = true;
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_remove,
                    removeData(cleanupWindow), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "cleanup failure retains token");
        expectFailure(() -> {
            try { transact(session, IWindowSession.Stub.TRANSACTION_relayout,
                    relayoutData(cleanupWindow), Parcel.obtain()); } catch (Exception error) {
                throw new RuntimeException(error);
            }
        }, "cleanup-failed token rejects relayout");
        DesktopWindowMetadataRegistry.failRemove = false;
        check(transact(session, IWindowSession.Stub.TRANSACTION_remove,
                removeData(cleanupWindow), Parcel.obtain()), "cleanup retry succeeds");
        pump();
    }

    public static void main(String[] args) throws Exception {
        openAndAuthentication();
        duplicateAndUnknownOperations();
        addFailureCleansForRetry();
        removeFailureRetainsRetiringToken();
        System.out.println("window manager endpoint fixture checks=" + checks);
    }
}

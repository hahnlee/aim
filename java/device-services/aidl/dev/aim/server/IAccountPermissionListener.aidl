package dev.aim.server;
import android.accounts.Account;
/** Committed explicit account grants; system UID only. */
oneway interface IAccountPermissionListener {
    void onAppPermissionChanged(in Account account, int uid);
}

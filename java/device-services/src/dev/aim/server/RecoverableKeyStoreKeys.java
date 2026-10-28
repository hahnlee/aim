/*
 * Copyright (C) 2017 The Android Open Source Project
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *     http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */
package dev.aim.server;

import static android.security.keystore.recovery.RecoveryController.ERROR_INSECURE_USER;
import static android.security.keystore.recovery.RecoveryController.ERROR_SERVICE_INTERNAL_ERROR;

import android.content.Context;
import android.os.Binder;
import android.os.RemoteException;
import android.os.ServiceSpecificException;
import android.os.UserHandle;
import android.security.keystore2.AndroidKeyStoreProvider;
import android.util.Log;

import com.android.server.locksettings.recoverablekeystore.InsecureUserException;
import com.android.server.locksettings.recoverablekeystore.PlatformEncryptionKey;
import com.android.server.locksettings.recoverablekeystore.PlatformKeyManager;
import com.android.server.locksettings.recoverablekeystore.RecoverableKeyGenerator;
import com.android.server.locksettings.recoverablekeystore.RecoverableKeyStorageException;
import com.android.server.locksettings.recoverablekeystore.storage.ApplicationKeyStorage;
import com.android.server.locksettings.recoverablekeystore.storage.CleanupManager;
import com.android.server.locksettings.recoverablekeystore.storage.RecoverableKeyStoreDb;
import com.android.server.locksettings.recoverablekeystore.storage.RecoverySnapshotStorage;

import java.io.IOException;
import java.security.InvalidKeyException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.util.Objects;

/**
 * Cold generateKey/removeKey scope of RecoverableKeyStoreManager.java,
 * frameworks/base at android-16.0.0_r1 (Apache-2.0), in the existing system process.
 * The native endpoint checks RECOVER_KEYSTORE before forwarding the authenticated UID;
 * this private bridge accepts only the system UID. Original platform-key, wrapping,
 * database, cleanup and application-key classes remain in services.jar. Only their
 * construction and these two method bodies are extracted, with explicit caller identity.
 */
final class RecoverableKeyStoreKeys extends IRecoverableKeyStoreKeys.Stub {
    private final Context context;
    private Keys keys;

    RecoverableKeyStoreKeys(Context context) { this.context = context; }

    private synchronized Keys keys() {
        if (keys == null) keys = new Keys(context);
        return keys;
    }

    @Override public String generateKey(int uid, String alias) throws RemoteException {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try { return keys().generateKey(uid, alias); }
        finally { Binder.restoreCallingIdentity(identity); }
    }

    @Override public void removeKey(int uid, String alias) throws RemoteException {
        Bridge.enforceSystemUid();
        long identity = Binder.clearCallingIdentity();
        try { keys().removeKey(uid, alias); }
        finally { Binder.restoreCallingIdentity(identity); }
    }

    private static final class Keys {
        private static final String TAG = "RecoverableKeyStoreMgr";
        private final RecoverableKeyStoreDb mDatabase;
        private final PlatformKeyManager mPlatformKeyManager;
        private final ApplicationKeyStorage mApplicationKeyStorage;
        private final CleanupManager mCleanupManager;
        private final RecoverableKeyGenerator mRecoverableKeyGenerator;

        Keys(Context context) {
            // aim-system starts directly from app_process, without ZygoteInit's provider preload.
            AndroidKeyStoreProvider.install();
            mDatabase = RecoverableKeyStoreDb.newInstance(context);
            try {
                mPlatformKeyManager = PlatformKeyManager.getInstance(context, mDatabase);
                mApplicationKeyStorage = ApplicationKeyStorage.getInstance();
            } catch (NoSuchAlgorithmException e) {
                throw new RuntimeException(e);
            } catch (KeyStoreException e) {
                throw new ServiceSpecificException(ERROR_SERVICE_INTERNAL_ERROR, e.getMessage());
            }
            mCleanupManager = CleanupManager.getInstance(context.getApplicationContext(),
                    RecoverySnapshotStorage.newInstance(), mDatabase, mApplicationKeyStorage);
            try {
                mCleanupManager.verifyKnownUsers();
            } catch (Exception e) {
                Log.e(TAG, "Failed to verify known users", e);
            }
            try {
                mRecoverableKeyGenerator = RecoverableKeyGenerator.newInstance(mDatabase);
            } catch (NoSuchAlgorithmException e) {
                Log.wtf(TAG, "AES keygen algorithm not available. AOSP must support this.", e);
                throw new ServiceSpecificException(ERROR_SERVICE_INTERNAL_ERROR, e.getMessage());
            }
        }

        String generateKey(int uid, String alias) throws RemoteException {
            int userId = UserHandle.getUserId(uid);
            mCleanupManager.registerRecoveryAgent(userId, uid);
            Objects.requireNonNull(alias, "alias is null");
            PlatformEncryptionKey encryptionKey;
            try {
                encryptionKey = mPlatformKeyManager.getEncryptKey(userId);
            } catch (NoSuchAlgorithmException e) {
                throw new RuntimeException(e);
            } catch (KeyStoreException | UnrecoverableKeyException | IOException e) {
                throw new ServiceSpecificException(ERROR_SERVICE_INTERNAL_ERROR, e.getMessage());
            } catch (InsecureUserException e) {
                throw new ServiceSpecificException(ERROR_INSECURE_USER, e.getMessage());
            }
            try {
                byte[] secretKey = mRecoverableKeyGenerator.generateAndStoreKey(encryptionKey,
                        userId, uid, alias, /*metadata=*/ null);
                mApplicationKeyStorage.setSymmetricKeyEntry(userId, uid, alias, secretKey);
                return mApplicationKeyStorage.getGrantAlias(userId, uid, alias);
            } catch (KeyStoreException | InvalidKeyException | RecoverableKeyStorageException e) {
                throw new ServiceSpecificException(ERROR_SERVICE_INTERNAL_ERROR, e.getMessage());
            }
        }

        void removeKey(int uid, String alias) {
            int userId = UserHandle.getUserId(uid);
            mCleanupManager.registerRecoveryAgent(userId, uid);
            Objects.requireNonNull(alias, "alias is null");
            boolean wasRemoved = mDatabase.removeKey(uid, alias);
            if (wasRemoved) {
                mDatabase.setShouldCreateSnapshot(userId, uid, true);
                mApplicationKeyStorage.deleteEntry(userId, uid, alias);
            }
        }
    }
}

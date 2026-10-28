/*
 * Copyright (C) 2022 The Android Open Source Project
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// android-16.0.0_r1; local changes: omit Android annotations and add a diagnostic message.
// android-16.0.0_r1 ArtModuleServiceInitializer's registration contract.
package com.android.server.art;
public final class ArtModuleServiceInitializer {
 private static final Object sLock = new Object();
 private static android.os.ArtModuleServiceManager sArtModuleServiceManager;
 public static void setArtModuleServiceManager(android.os.ArtModuleServiceManager manager) {
  synchronized (sLock) {
   if (sArtModuleServiceManager != null) throw new IllegalStateException("ArtModuleServiceManager is already set");
   sArtModuleServiceManager = manager;
  }
 }
 public static android.os.ArtModuleServiceManager getArtModuleServiceManager() {
  synchronized (sLock) {
   return java.util.Objects.requireNonNull(sArtModuleServiceManager, "ArtModuleServiceManager");
  }
 }
}

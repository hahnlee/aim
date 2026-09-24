/*
 * Copyright (c) 2008, 2009, Oracle and/or its affiliates. All rights reserved.
 * DO NOT ALTER OR REMOVE COPYRIGHT NOTICES OR THIS FILE HEADER.
 *
 * This code is free software; you can redistribute it and/or modify it
 * under the terms of the GNU General Public License version 2 only, as
 * published by the Free Software Foundation.  Oracle designates this
 * particular file as subject to the "Classpath" exception as provided
 * by Oracle in the LICENSE file that accompanied this code.
 *
 * This code is distributed in the hope that it will be useful, but WITHOUT
 * ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
 * FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License
 * version 2 for more details (a copy is included in the LICENSE file that
 * accompanied this code).
 *
 * You should have received a copy of the GNU General Public License version
 * 2 along with this work; if not, write to the Free Software Foundation,
 * Inc., 51 Franklin St, Fifth Floor, Boston, MA 02110-1301 USA.
 *
 * Please contact Oracle, 500 Oracle Parkway, Redwood Shores, CA 94065 USA
 * or visit www.oracle.com if you need additional information or have any
 * questions.
 */

/*
 * SPDX-License-Identifier: GPL-2.0-only WITH Classpath-exception-2.0
 * Darwin ART modifications: Copyright 2026 Darwin ART contributors.
 * Adapted from platform/libcore at 080fac8bb8670bc7fbc895050caf4b13c4d6cd12,
 * ojluni/src/main/native/UnixCopyFile.c, for virtual descriptor routing and
 * copy-loop handling. Original notice restored on 2026-09-24.
 * The Classpath exception is retained for these modifications.
 * See ../../licensing/OPENJDK.md and the complete license/exception text in
 * ../../licensing/third-party/libcore-ojluni-LICENSE.
 */

#include "jni.h"
#include "jni_util.h"
#include "jlong.h"

#include <errno.h>
#include <stdint.h>
#include <unistd.h>

#include "sun_nio_fs_UnixCopyFile.h"

#define RESTARTABLE(command, result) \
  do {                                 \
    do {                               \
      (result) = (command);            \
    } while ((result) == -1 && errno == EINTR); \
  } while (0)

static void throw_unix_exception(JNIEnv* env, int error_number) {
  jobject exception = JNU_NewObjectByName(
      env, "sun/nio/fs/UnixException", "(I)V", error_number);
  if (exception != NULL) (*env)->Throw(env, exception);
}

// Keep the upstream UnixCopyFile native contract. The read/write calls are
// redirected by darwin_openjdk_nio_fs_redirect.h, so Android virtual FDs are
// copied through the Bionic descriptor table without a host-side alias.
JNIEXPORT void JNICALL Java_sun_nio_fs_UnixCopyFile_transfer(
    JNIEnv* env, jclass unused, jint destination, jint source,
    jlong cancel_address) {
  char buffer[8192];
  volatile jint* cancel = (volatile jint*)jlong_to_ptr(cancel_address);
  for (;;) {
    ssize_t count;
    RESTARTABLE(read((int)source, buffer, sizeof(buffer)), count);
    if (count <= 0) {
      if (count < 0) throw_unix_exception(env, errno);
      return;
    }
    if (cancel != NULL && *cancel != 0) {
      throw_unix_exception(env, ECANCELED);
      return;
    }
    ssize_t offset = 0;
    while (offset < count) {
      ssize_t written;
      RESTARTABLE(write((int)destination, buffer + offset,
                        (size_t)(count - offset)), written);
      if (written <= 0) {
        if (written < 0) throw_unix_exception(env, errno);
        return;
      }
      offset += written;
    }
  }
}

#undef RESTARTABLE

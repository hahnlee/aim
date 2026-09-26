#include "darwin_framework_natives.h"

#include <sqlite3.h>

#include <algorithm>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iterator>
#include <string>
#include <utility>
#include <variant>
#include <vector>

extern "C" intptr_t darwin_art_bionic_fs_resolve_private_host_path(
    const char* path, char* output, size_t capacity);

namespace {

using Cell = std::variant<std::monostate, std::int64_t, double, std::string,
                          std::vector<std::uint8_t>>;

struct DarwinCursorWindow {
  std::string name;
  int columns = 0;
  std::vector<std::vector<Cell>> rows;
};

sqlite3* Database(jlong pointer) {
  return reinterpret_cast<sqlite3*>(static_cast<std::uintptr_t>(pointer));
}

sqlite3_stmt* Statement(jlong pointer) {
  return reinterpret_cast<sqlite3_stmt*>(static_cast<std::uintptr_t>(pointer));
}

DarwinCursorWindow* Window(jlong pointer) {
  return reinterpret_cast<DarwinCursorWindow*>(
      static_cast<std::uintptr_t>(pointer));
}

void ThrowSqlite(JNIEnv* env, sqlite3* database, int status,
                 const char* operation) {
  if (env->ExceptionCheck()) return;
  std::string message(operation);
  message += ": ";
  message += database == nullptr ? sqlite3_errstr(status)
                                 : sqlite3_errmsg(database);
  jclass exception = env->FindClass("android/database/sqlite/SQLiteException");
  if (exception == nullptr) {
    env->ExceptionClear();
    exception = env->FindClass("java/lang/RuntimeException");
  }
  if (exception != nullptr) {
    env->ThrowNew(exception, message.c_str());
    env->DeleteLocalRef(exception);
  }
}

std::string Utf8(JNIEnv* env, jstring value) {
  if (value == nullptr) return {};
  const char* characters = env->GetStringUTFChars(value, nullptr);
  if (characters == nullptr) return {};
  std::string result(characters);
  env->ReleaseStringUTFChars(value, characters);
  return result;
}

bool StepToCompletion(JNIEnv* env, sqlite3* database, sqlite3_stmt* statement,
                      const char* operation) {
  int status = SQLITE_OK;
  do {
    status = sqlite3_step(statement);
  } while (status == SQLITE_ROW);
  if (status != SQLITE_DONE) {
    ThrowSqlite(env, database, status, operation);
    sqlite3_reset(statement);
    return false;
  }
  sqlite3_reset(statement);
  return true;
}

// Resolves a guest database name to its host backing path, or returns false
// when the name is not under a private /data mount.
bool ResolveGuestDatabasePath(const char* guest, std::string* host) {
  const intptr_t length =
      darwin_art_bionic_fs_resolve_private_host_path(guest, nullptr, 0);
  if (length < 0) return false;
  host->resize(static_cast<std::size_t>(length) + 1);
  if (darwin_art_bionic_fs_resolve_private_host_path(guest, host->data(),
                                                     host->size()) != length) {
    return false;
  }
  host->resize(static_cast<std::size_t>(length));
  return true;
}

// The connection's VFS: the platform default with guest database names
// resolved to host paths. SQLite passes every name the app gives it, the
// nativeOpen path and ATTACH DATABASE's (AccountsDb attaches accounts_ce.db
// to the DE connection), through xFullPathname before opening it, and derives
// journal and WAL names from the result, so resolving there keeps those names
// on the host too. The copy keeps the platform VFS's pAppData, which its
// xOpen reads.
sqlite3_vfs* platform_vfs = nullptr;

sqlite3_vfs* GuestPathVfs() {
  static sqlite3_vfs* vfs = [] {
    platform_vfs = sqlite3_vfs_find(nullptr);
    if (platform_vfs == nullptr) return static_cast<sqlite3_vfs*>(nullptr);
    static sqlite3_vfs guest = *platform_vfs;
    guest.zName = "darwin-art-guest";
    guest.xFullPathname = [](sqlite3_vfs*, const char* name, int size,
                             char* output) -> int {
      std::string host;
      if (!ResolveGuestDatabasePath(name, &host)) return SQLITE_CANTOPEN;
      return platform_vfs->xFullPathname(platform_vfs, host.c_str(), size, output);
    };
    if (sqlite3_vfs_register(&guest, 0) != SQLITE_OK) return static_cast<sqlite3_vfs*>(nullptr);
    return &guest;
  }();
  return vfs;
}

jlong SqliteOpen(JNIEnv* env, jclass, jstring path, jint open_flags, jstring,
                 jboolean, jboolean, jint, jint) {
  const std::string guest_path = Utf8(env, path);
  sqlite3_vfs* vfs = GuestPathVfs();
  if (vfs == nullptr) {
    ThrowSqlite(env, nullptr, SQLITE_CANTOPEN, "register database VFS");
    return 0;
  }
  int flags = (open_flags & 1) != 0 ? SQLITE_OPEN_READONLY
                                    : SQLITE_OPEN_READWRITE;
  if ((open_flags & 0x10000000) != 0) flags |= SQLITE_OPEN_CREATE;
  flags |= SQLITE_OPEN_FULLMUTEX;
  sqlite3* database = nullptr;
  const int status = sqlite3_open_v2(guest_path.c_str(), &database, flags, vfs->zName);
  if (status != SQLITE_OK) {
    ThrowSqlite(env, database, status, "open database");
    if (database != nullptr) sqlite3_close_v2(database);
    return 0;
  }
  sqlite3_busy_timeout(database, 2500);
  return reinterpret_cast<jlong>(database);
}

void SqliteClose(JNIEnv*, jclass, jlong connection, jboolean) {
  if (sqlite3* database = Database(connection); database != nullptr) {
    sqlite3_close_v2(database);
  }
}

jlong SqlitePrepare(JNIEnv* env, jclass, jlong connection, jstring sql) {
  sqlite3* database = Database(connection);
  const std::string native_sql = Utf8(env, sql);
  sqlite3_stmt* statement = nullptr;
  const int status = sqlite3_prepare_v2(database, native_sql.c_str(), -1,
                                        &statement, nullptr);
  if (status != SQLITE_OK) {
    ThrowSqlite(env, database, status, "prepare statement");
    return 0;
  }
  return reinterpret_cast<jlong>(statement);
}

void SqliteFinalize(JNIEnv*, jclass, jlong, jlong statement) {
  if (sqlite3_stmt* value = Statement(statement); value != nullptr) {
    sqlite3_finalize(value);
  }
}

void SqliteReset(JNIEnv* env, jclass, jlong connection, jlong statement) {
  sqlite3_stmt* value = Statement(statement);
  const int reset = sqlite3_reset(value);
  const int clear = sqlite3_clear_bindings(value);
  if (reset != SQLITE_OK || clear != SQLITE_OK) {
    ThrowSqlite(env, Database(connection), reset != SQLITE_OK ? reset : clear,
                "reset statement");
  }
}

void SqliteBindNull(JNIEnv* env, jclass, jlong connection, jlong statement,
                    jint index) {
  const int status = sqlite3_bind_null(Statement(statement), index);
  if (status != SQLITE_OK) ThrowSqlite(env, Database(connection), status, "bind null");
}

void SqliteBindLong(JNIEnv* env, jclass, jlong connection, jlong statement,
                    jint index, jlong value) {
  const int status = sqlite3_bind_int64(Statement(statement), index, value);
  if (status != SQLITE_OK) ThrowSqlite(env, Database(connection), status, "bind long");
}

void SqliteBindDouble(JNIEnv* env, jclass, jlong connection, jlong statement,
                      jint index, jdouble value) {
  const int status = sqlite3_bind_double(Statement(statement), index, value);
  if (status != SQLITE_OK) ThrowSqlite(env, Database(connection), status, "bind double");
}

void SqliteBindString(JNIEnv* env, jclass, jlong connection, jlong statement,
                      jint index, jstring value) {
  const std::string text = Utf8(env, value);
  const int status = sqlite3_bind_text(Statement(statement), index, text.data(),
                                       static_cast<int>(text.size()), SQLITE_TRANSIENT);
  if (status != SQLITE_OK) ThrowSqlite(env, Database(connection), status, "bind string");
}

void SqliteBindBlob(JNIEnv* env, jclass, jlong connection, jlong statement,
                    jint index, jbyteArray value) {
  const jsize size = value == nullptr ? 0 : env->GetArrayLength(value);
  jbyte* bytes = value == nullptr ? nullptr : env->GetByteArrayElements(value, nullptr);
  if (value != nullptr && bytes == nullptr) return;
  const int status = sqlite3_bind_blob(Statement(statement), index, bytes, size,
                                       SQLITE_TRANSIENT);
  if (bytes != nullptr) env->ReleaseByteArrayElements(value, bytes, JNI_ABORT);
  if (status != SQLITE_OK) ThrowSqlite(env, Database(connection), status, "bind blob");
}

void SqliteExecute(JNIEnv* env, jclass, jlong connection, jlong statement,
                   jboolean) {
  StepToCompletion(env, Database(connection), Statement(statement), "execute statement");
}

jlong SqliteExecuteForLong(JNIEnv* env, jclass, jlong connection,
                           jlong statement) {
  sqlite3* database = Database(connection);
  sqlite3_stmt* value = Statement(statement);
  const int status = sqlite3_step(value);
  if (status != SQLITE_ROW) {
    ThrowSqlite(env, database, status, "execute scalar query");
    sqlite3_reset(value);
    return 0;
  }
  const jlong result = sqlite3_column_int64(value, 0);
  sqlite3_reset(value);
  return result;
}

jstring SqliteExecuteForString(JNIEnv* env, jclass, jlong connection,
                               jlong statement) {
  sqlite3* database = Database(connection);
  sqlite3_stmt* value = Statement(statement);
  const int status = sqlite3_step(value);
  if (status != SQLITE_ROW) {
    ThrowSqlite(env, database, status, "execute string query");
    sqlite3_reset(value);
    return nullptr;
  }
  const unsigned char* text = sqlite3_column_text(value, 0);
  jstring result = text == nullptr
                       ? nullptr
                       : env->NewStringUTF(reinterpret_cast<const char*>(text));
  sqlite3_reset(value);
  return result;
}

jint SqliteExecuteForChangedRowCount(JNIEnv* env, jclass, jlong connection,
                                     jlong statement) {
  sqlite3* database = Database(connection);
  if (!StepToCompletion(env, database, Statement(statement), "execute update")) return -1;
  return sqlite3_changes(database);
}

jlong SqliteExecuteForLastInsertedRowId(JNIEnv* env, jclass, jlong connection,
                                        jlong statement) {
  sqlite3* database = Database(connection);
  if (!StepToCompletion(env, database, Statement(statement), "execute insert")) return -1;
  return sqlite3_last_insert_rowid(database);
}

jint SqliteGetParameterCount(JNIEnv*, jclass, jlong, jlong statement) {
  return sqlite3_bind_parameter_count(Statement(statement));
}

jboolean SqliteIsReadOnly(JNIEnv*, jclass, jlong, jlong statement) {
  return sqlite3_stmt_readonly(Statement(statement)) != 0;
}

jint SqliteGetColumnCount(JNIEnv*, jclass, jlong, jlong statement) {
  return sqlite3_column_count(Statement(statement));
}

jstring SqliteGetColumnName(JNIEnv* env, jclass, jlong, jlong statement,
                            jint column) {
  const char* name = sqlite3_column_name(Statement(statement), column);
  return name == nullptr ? nullptr : env->NewStringUTF(name);
}

jlong SqliteChanges(JNIEnv*, jclass, jlong connection) {
  return sqlite3_changes64(Database(connection));
}

jlong SqliteTotalChanges(JNIEnv*, jclass, jlong connection) {
  return sqlite3_total_changes64(Database(connection));
}

jint SqliteLastInsertRowId(JNIEnv*, jclass, jlong connection) {
  return static_cast<jint>(sqlite3_last_insert_rowid(Database(connection)));
}

void SqliteCancel(JNIEnv*, jclass, jlong connection) {
  sqlite3_interrupt(Database(connection));
}

void SqliteResetCancel(JNIEnv*, jclass, jlong, jboolean) {}
void SqliteRegisterFunction(JNIEnv*, jclass, jlong, jstring, jobject) {}
int CompareUtf8(void*, int left_size, const void* left, int right_size,
                const void* right) {
  const int shared_size = std::min(left_size, right_size);
  const int compared = std::memcmp(left, right, shared_size);
  if (compared != 0) return compared;
  return left_size < right_size ? -1 : left_size > right_size ? 1 : 0;
}

void SqliteRegisterLocalizedCollators(JNIEnv* env, jclass, jlong connection,
                                      jstring) {
  sqlite3* database = Database(connection);
  for (const char* name : {"LOCALIZED", "UNICODE"}) {
    const int status = sqlite3_create_collation(database, name, SQLITE_UTF8,
                                                nullptr, &CompareUtf8);
    if (status != SQLITE_OK) {
      ThrowSqlite(env, database, status, "register localized collation");
      return;
    }
  }
}
jint SqliteGetDbLookaside(JNIEnv*, jclass, jlong) { return 0; }
jboolean SqliteUpdatesTempOnly(JNIEnv*, jclass, jlong, jlong) { return JNI_FALSE; }
jint SqliteExecuteForBlobFileDescriptor(JNIEnv*, jclass, jlong, jlong) { return -1; }

Cell ColumnCell(sqlite3_stmt* statement, int column) {
  switch (sqlite3_column_type(statement, column)) {
    case SQLITE_INTEGER:
      return static_cast<std::int64_t>(sqlite3_column_int64(statement, column));
    case SQLITE_FLOAT:
      return sqlite3_column_double(statement, column);
    case SQLITE_TEXT: {
      const char* text = reinterpret_cast<const char*>(sqlite3_column_text(statement, column));
      const int size = sqlite3_column_bytes(statement, column);
      return std::string(text == nullptr ? "" : text, std::max(0, size));
    }
    case SQLITE_BLOB: {
      const auto* bytes = static_cast<const std::uint8_t*>(sqlite3_column_blob(statement, column));
      const int size = sqlite3_column_bytes(statement, column);
      return bytes == nullptr ? std::vector<std::uint8_t>{}
                              : std::vector<std::uint8_t>(bytes, bytes + std::max(0, size));
    }
    default:
      return std::monostate{};
  }
}

jlong SqliteExecuteForCursorWindow(JNIEnv* env, jclass, jlong connection,
                                   jlong statement, jlong window_pointer,
                                   jint start_position, jint, jboolean count_all) {
  sqlite3* database = Database(connection);
  sqlite3_stmt* value = Statement(statement);
  DarwinCursorWindow* window = Window(window_pointer);
  if (window == nullptr) return 0;
  window->rows.clear();
  window->columns = sqlite3_column_count(value);
  int total = 0;
  int status = SQLITE_OK;
  while ((status = sqlite3_step(value)) == SQLITE_ROW) {
    if (total >= start_position) {
      std::vector<Cell> row;
      row.reserve(window->columns);
      for (int column = 0; column < window->columns; ++column) {
        row.push_back(ColumnCell(value, column));
      }
      window->rows.push_back(std::move(row));
    }
    ++total;
    if (!count_all && window->rows.size() >= 4096) break;
  }
  if (status != SQLITE_DONE && status != SQLITE_ROW) {
    ThrowSqlite(env, database, status, "execute cursor query");
  }
  sqlite3_reset(value);
  return (static_cast<jlong>(start_position) << 32) |
         static_cast<std::uint32_t>(total);
}

jlong CursorCreate(JNIEnv* env, jclass, jstring name, jint) {
  auto* window = new DarwinCursorWindow;
  window->name = Utf8(env, name);
  return reinterpret_cast<jlong>(window);
}

void CursorDispose(JNIEnv*, jclass, jlong pointer) { delete Window(pointer); }
void CursorClear(JNIEnv*, jclass, jlong pointer) { Window(pointer)->rows.clear(); }
jstring CursorGetName(JNIEnv* env, jclass, jlong pointer) {
  return env->NewStringUTF(Window(pointer)->name.c_str());
}
jint CursorGetNumRows(JNIEnv*, jclass, jlong pointer) {
  return static_cast<jint>(Window(pointer)->rows.size());
}
jboolean CursorSetNumColumns(JNIEnv*, jclass, jlong pointer, jint columns) {
  Window(pointer)->columns = columns;
  return JNI_TRUE;
}
jboolean CursorAllocRow(JNIEnv*, jclass, jlong pointer) {
  DarwinCursorWindow* window = Window(pointer);
  window->rows.emplace_back(static_cast<std::size_t>(window->columns));
  return JNI_TRUE;
}
void CursorFreeLastRow(JNIEnv*, jclass, jlong pointer) {
  DarwinCursorWindow* window = Window(pointer);
  if (!window->rows.empty()) window->rows.pop_back();
}

Cell* CursorCell(jlong pointer, jint row, jint column) {
  DarwinCursorWindow* window = Window(pointer);
  if (window == nullptr || row < 0 || column < 0 ||
      static_cast<std::size_t>(row) >= window->rows.size() ||
      static_cast<std::size_t>(column) >= window->rows[row].size()) return nullptr;
  return &window->rows[row][column];
}

jint CursorGetType(JNIEnv*, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column);
  if (cell == nullptr || std::holds_alternative<std::monostate>(*cell)) return 0;
  if (std::holds_alternative<std::int64_t>(*cell)) return 1;
  if (std::holds_alternative<double>(*cell)) return 2;
  if (std::holds_alternative<std::string>(*cell)) return 3;
  return 4;
}

jlong CursorGetLong(JNIEnv*, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column);
  if (cell == nullptr) return 0;
  if (auto* value = std::get_if<std::int64_t>(cell)) return *value;
  if (auto* value = std::get_if<double>(cell)) return static_cast<jlong>(*value);
  if (auto* value = std::get_if<std::string>(cell)) return std::strtoll(value->c_str(), nullptr, 10);
  return 0;
}

jdouble CursorGetDouble(JNIEnv*, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column);
  if (cell == nullptr) return 0;
  if (auto* value = std::get_if<double>(cell)) return *value;
  if (auto* value = std::get_if<std::int64_t>(cell)) return static_cast<double>(*value);
  if (auto* value = std::get_if<std::string>(cell)) return std::strtod(value->c_str(), nullptr);
  return 0;
}

jstring CursorGetString(JNIEnv* env, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column);
  if (cell == nullptr || std::holds_alternative<std::monostate>(*cell)) return nullptr;
  std::string text;
  if (auto* value = std::get_if<std::string>(cell)) text = *value;
  else if (auto* value = std::get_if<std::int64_t>(cell)) text = std::to_string(*value);
  else if (auto* value = std::get_if<double>(cell)) text = std::to_string(*value);
  return env->NewStringUTF(text.c_str());
}

jbyteArray CursorGetBlob(JNIEnv* env, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column);
  auto* blob = cell == nullptr ? nullptr : std::get_if<std::vector<std::uint8_t>>(cell);
  if (blob == nullptr) return nullptr;
  jbyteArray result = env->NewByteArray(static_cast<jsize>(blob->size()));
  if (result != nullptr && !blob->empty()) {
    env->SetByteArrayRegion(result, 0, static_cast<jsize>(blob->size()),
                            reinterpret_cast<const jbyte*>(blob->data()));
  }
  return result;
}

jboolean CursorPutNull(JNIEnv*, jclass, jlong pointer, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column); if (cell == nullptr) return JNI_FALSE;
  *cell = std::monostate{}; return JNI_TRUE;
}
jboolean CursorPutLong(JNIEnv*, jclass, jlong pointer, jlong value, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column); if (cell == nullptr) return JNI_FALSE;
  *cell = static_cast<std::int64_t>(value); return JNI_TRUE;
}
jboolean CursorPutDouble(JNIEnv*, jclass, jlong pointer, jdouble value, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column); if (cell == nullptr) return JNI_FALSE;
  *cell = value; return JNI_TRUE;
}
jboolean CursorPutString(JNIEnv* env, jclass, jlong pointer, jstring value, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column); if (cell == nullptr) return JNI_FALSE;
  *cell = Utf8(env, value); return JNI_TRUE;
}
jboolean CursorPutBlob(JNIEnv* env, jclass, jlong pointer, jbyteArray value, jint row, jint column) {
  Cell* cell = CursorCell(pointer, row, column); if (cell == nullptr) return JNI_FALSE;
  const jsize size = value == nullptr ? 0 : env->GetArrayLength(value);
  std::vector<std::uint8_t> bytes(static_cast<std::size_t>(size));
  if (size > 0) env->GetByteArrayRegion(value, 0, size, reinterpret_cast<jbyte*>(bytes.data()));
  *cell = std::move(bytes); return JNI_TRUE;
}

jlong CursorCreateFromParcel(JNIEnv* env, jclass klass, jobject) {
  return CursorCreate(env, klass, nullptr, 0);
}
void CursorWriteToParcel(JNIEnv*, jclass, jlong, jobject) {}
void CursorCopyStringToBuffer(JNIEnv*, jclass, jlong, jint, jint, jobject) {}

bool Register(JNIEnv* env, const char* name, JNINativeMethod* methods, jint count) {
  jclass klass = env->FindClass(name);
  if (klass == nullptr) return false;
  const bool ok = env->RegisterNatives(klass, methods, count) == JNI_OK;
  env->DeleteLocalRef(klass);
  return ok;
}

}  // namespace

namespace darwin_art {

bool RegisterFrameworkSqliteNatives(JNIEnv* env) {
  JNINativeMethod connection[] = {
      {const_cast<char*>("nativeOpen"), const_cast<char*>("(Ljava/lang/String;ILjava/lang/String;ZZII)J"), reinterpret_cast<void*>(&SqliteOpen)},
      {const_cast<char*>("nativeClose"), const_cast<char*>("(JZ)V"), reinterpret_cast<void*>(&SqliteClose)},
      {const_cast<char*>("nativePrepareStatement"), const_cast<char*>("(JLjava/lang/String;)J"), reinterpret_cast<void*>(&SqlitePrepare)},
      {const_cast<char*>("nativeFinalizeStatement"), const_cast<char*>("(JJ)V"), reinterpret_cast<void*>(&SqliteFinalize)},
      {const_cast<char*>("nativeGetParameterCount"), const_cast<char*>("(JJ)I"), reinterpret_cast<void*>(&SqliteGetParameterCount)},
      {const_cast<char*>("nativeIsReadOnly"), const_cast<char*>("(JJ)Z"), reinterpret_cast<void*>(&SqliteIsReadOnly)},
      {const_cast<char*>("nativeGetColumnCount"), const_cast<char*>("(JJ)I"), reinterpret_cast<void*>(&SqliteGetColumnCount)},
      {const_cast<char*>("nativeGetColumnName"), const_cast<char*>("(JJI)Ljava/lang/String;"), reinterpret_cast<void*>(&SqliteGetColumnName)},
      {const_cast<char*>("nativeBindNull"), const_cast<char*>("(JJI)V"), reinterpret_cast<void*>(&SqliteBindNull)},
      {const_cast<char*>("nativeBindLong"), const_cast<char*>("(JJIJ)V"), reinterpret_cast<void*>(&SqliteBindLong)},
      {const_cast<char*>("nativeBindDouble"), const_cast<char*>("(JJID)V"), reinterpret_cast<void*>(&SqliteBindDouble)},
      {const_cast<char*>("nativeBindString"), const_cast<char*>("(JJILjava/lang/String;)V"), reinterpret_cast<void*>(&SqliteBindString)},
      {const_cast<char*>("nativeBindBlob"), const_cast<char*>("(JJI[B)V"), reinterpret_cast<void*>(&SqliteBindBlob)},
      {const_cast<char*>("nativeResetStatementAndClearBindings"), const_cast<char*>("(JJ)V"), reinterpret_cast<void*>(&SqliteReset)},
      {const_cast<char*>("nativeExecute"), const_cast<char*>("(JJZ)V"), reinterpret_cast<void*>(&SqliteExecute)},
      {const_cast<char*>("nativeExecuteForLong"), const_cast<char*>("(JJ)J"), reinterpret_cast<void*>(&SqliteExecuteForLong)},
      {const_cast<char*>("nativeExecuteForString"), const_cast<char*>("(JJ)Ljava/lang/String;"), reinterpret_cast<void*>(&SqliteExecuteForString)},
      {const_cast<char*>("nativeExecuteForChangedRowCount"), const_cast<char*>("(JJ)I"), reinterpret_cast<void*>(&SqliteExecuteForChangedRowCount)},
      {const_cast<char*>("nativeExecuteForLastInsertedRowId"), const_cast<char*>("(JJ)J"), reinterpret_cast<void*>(&SqliteExecuteForLastInsertedRowId)},
      {const_cast<char*>("nativeExecuteForCursorWindow"), const_cast<char*>("(JJJIIZ)J"), reinterpret_cast<void*>(&SqliteExecuteForCursorWindow)},
      {const_cast<char*>("nativeExecuteForBlobFileDescriptor"), const_cast<char*>("(JJ)I"), reinterpret_cast<void*>(&SqliteExecuteForBlobFileDescriptor)},
      {const_cast<char*>("nativeGetDbLookaside"), const_cast<char*>("(J)I"), reinterpret_cast<void*>(&SqliteGetDbLookaside)},
      {const_cast<char*>("nativeCancel"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(&SqliteCancel)},
      {const_cast<char*>("nativeResetCancel"), const_cast<char*>("(JZ)V"), reinterpret_cast<void*>(&SqliteResetCancel)},
      {const_cast<char*>("nativeRegisterLocalizedCollators"), const_cast<char*>("(JLjava/lang/String;)V"), reinterpret_cast<void*>(&SqliteRegisterLocalizedCollators)},
      {const_cast<char*>("nativeRegisterCustomScalarFunction"), const_cast<char*>("(JLjava/lang/String;Ljava/util/function/UnaryOperator;)V"), reinterpret_cast<void*>(&SqliteRegisterFunction)},
      {const_cast<char*>("nativeRegisterCustomAggregateFunction"), const_cast<char*>("(JLjava/lang/String;Ljava/util/function/BinaryOperator;)V"), reinterpret_cast<void*>(&SqliteRegisterFunction)},
      {const_cast<char*>("nativeChanges"), const_cast<char*>("(J)J"), reinterpret_cast<void*>(&SqliteChanges)},
      {const_cast<char*>("nativeTotalChanges"), const_cast<char*>("(J)J"), reinterpret_cast<void*>(&SqliteTotalChanges)},
      {const_cast<char*>("nativeLastInsertRowId"), const_cast<char*>("(J)I"), reinterpret_cast<void*>(&SqliteLastInsertRowId)},
      {const_cast<char*>("nativeUpdatesTempOnly"), const_cast<char*>("(JJ)Z"), reinterpret_cast<void*>(&SqliteUpdatesTempOnly)},
  };
  if (!Register(env, "android/database/sqlite/SQLiteConnection", connection,
                static_cast<jint>(std::size(connection)))) return false;

  JNINativeMethod cursor[] = {
      {const_cast<char*>("nativeCreate"), const_cast<char*>("(Ljava/lang/String;I)J"), reinterpret_cast<void*>(&CursorCreate)},
      {const_cast<char*>("nativeCreateFromParcel"), const_cast<char*>("(Landroid/os/Parcel;)J"), reinterpret_cast<void*>(&CursorCreateFromParcel)},
      {const_cast<char*>("nativeDispose"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(&CursorDispose)},
      {const_cast<char*>("nativeClear"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(&CursorClear)},
      {const_cast<char*>("nativeGetName"), const_cast<char*>("(J)Ljava/lang/String;"), reinterpret_cast<void*>(&CursorGetName)},
      {const_cast<char*>("nativeGetNumRows"), const_cast<char*>("(J)I"), reinterpret_cast<void*>(&CursorGetNumRows)},
      {const_cast<char*>("nativeSetNumColumns"), const_cast<char*>("(JI)Z"), reinterpret_cast<void*>(&CursorSetNumColumns)},
      {const_cast<char*>("nativeAllocRow"), const_cast<char*>("(J)Z"), reinterpret_cast<void*>(&CursorAllocRow)},
      {const_cast<char*>("nativeFreeLastRow"), const_cast<char*>("(J)V"), reinterpret_cast<void*>(&CursorFreeLastRow)},
      {const_cast<char*>("nativeGetType"), const_cast<char*>("(JII)I"), reinterpret_cast<void*>(&CursorGetType)},
      {const_cast<char*>("nativeGetLong"), const_cast<char*>("(JII)J"), reinterpret_cast<void*>(&CursorGetLong)},
      {const_cast<char*>("nativeGetDouble"), const_cast<char*>("(JII)D"), reinterpret_cast<void*>(&CursorGetDouble)},
      {const_cast<char*>("nativeGetString"), const_cast<char*>("(JII)Ljava/lang/String;"), reinterpret_cast<void*>(&CursorGetString)},
      {const_cast<char*>("nativeGetBlob"), const_cast<char*>("(JII)[B"), reinterpret_cast<void*>(&CursorGetBlob)},
      {const_cast<char*>("nativePutNull"), const_cast<char*>("(JII)Z"), reinterpret_cast<void*>(&CursorPutNull)},
      {const_cast<char*>("nativePutLong"), const_cast<char*>("(JJII)Z"), reinterpret_cast<void*>(&CursorPutLong)},
      {const_cast<char*>("nativePutDouble"), const_cast<char*>("(JDII)Z"), reinterpret_cast<void*>(&CursorPutDouble)},
      {const_cast<char*>("nativePutString"), const_cast<char*>("(JLjava/lang/String;II)Z"), reinterpret_cast<void*>(&CursorPutString)},
      {const_cast<char*>("nativePutBlob"), const_cast<char*>("(J[BII)Z"), reinterpret_cast<void*>(&CursorPutBlob)},
      {const_cast<char*>("nativeCopyStringToBuffer"), const_cast<char*>("(JIILandroid/database/CharArrayBuffer;)V"), reinterpret_cast<void*>(&CursorCopyStringToBuffer)},
      {const_cast<char*>("nativeWriteToParcel"), const_cast<char*>("(JLandroid/os/Parcel;)V"), reinterpret_cast<void*>(&CursorWriteToParcel)},
  };
  return Register(env, "android/database/CursorWindow", cursor,
                  static_cast<jint>(std::size(cursor)));
}

}  // namespace darwin_art

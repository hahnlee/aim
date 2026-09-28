#include <cstdio>
#include <cstring>
#include <iostream>

#include <ziparchive/zip_archive.h>
#include <ziparchive/zip_writer.h>

int main() {
  FILE* file = std::tmpfile();
  if (file == nullptr) {
    return 1;
  }

  constexpr char kPayload[] = "aim";
  ZipWriter writer(file);
  if (writer.StartEntry("probe.txt", 0) != 0 ||
      writer.WriteBytes(kPayload, sizeof(kPayload) - 1) != 0 ||
      writer.FinishEntry() != 0 || writer.Finish() != 0) {
    std::fclose(file);
    return 2;
  }

  ZipArchiveHandle archive = nullptr;
  if (OpenArchiveFd(fileno(file), "aim-memory-zip", &archive, false) !=
      0) {
    std::fclose(file);
    return 3;
  }
  ZipEntry64 entry;
  if (FindEntry(archive, "probe.txt", &entry) != 0 ||
      entry.uncompressed_length != sizeof(kPayload) - 1) {
    CloseArchive(archive);
    std::fclose(file);
    return 4;
  }
  uint8_t extracted[sizeof(kPayload)] = {};
  if (ExtractToMemory(archive, &entry, extracted, sizeof(extracted) - 1) != 0 ||
      std::strcmp(reinterpret_cast<const char*>(extracted), kPayload) != 0) {
    CloseArchive(archive);
    std::fclose(file);
    return 5;
  }
  CloseArchive(archive);
  std::fclose(file);
  std::cout << "ziparchive-for-incfs: writer+reader payload=aim\n";
  return 0;
}

#pragma once

#if defined(__APPLE__)

#include <mach-o/dyld.h>
#include <mach-o/dyld_images.h>
#include <mach-o/loader.h>
#include <mach/mach.h>
#include <mach/mach_vm.h>

#include "darwin_remote_task_cache.h"

#include <cstdint>
#include <cstring>
#include <cstdio>
#include <functional>
#include <limits>
#include <string>
#include <vector>
#include <unistd.h>

namespace android::procinfo {

struct MapInfo {
  uint64_t start = 0;
  uint64_t end = 0;
  uint64_t pgoff = 0;
  uint64_t flags = 0;
  std::string name;
};

struct ImageSegment {
  uint64_t start;
  uint64_t end;
  uint64_t file_offset;
  std::string name;
};

inline std::vector<ImageSegment> DarwinImageSegments() {
  std::vector<ImageSegment> segments;
  for (uint32_t image = 0; image < _dyld_image_count(); ++image) {
    const mach_header_64* header = reinterpret_cast<const mach_header_64*>(_dyld_get_image_header(image));
    if (header == nullptr || header->magic != MH_MAGIC_64) continue;
    const intptr_t slide = _dyld_get_image_vmaddr_slide(image);
    const char* image_name = _dyld_get_image_name(image);
    const uint8_t* command_bytes = reinterpret_cast<const uint8_t*>(header + 1);
    for (uint32_t command_index = 0; command_index < header->ncmds; ++command_index) {
      const load_command* command = reinterpret_cast<const load_command*>(command_bytes);
      if (command->cmd == LC_SEGMENT_64) {
        const auto* segment = reinterpret_cast<const segment_command_64*>(command);
        if (segment->vmsize != 0) {
          segments.push_back({static_cast<uint64_t>(segment->vmaddr + slide),
                              static_cast<uint64_t>(segment->vmaddr + slide + segment->vmsize),
                              segment->fileoff, image_name == nullptr ? "" : image_name});
        }
      }
      command_bytes += command->cmdsize;
    }
  }
  return segments;
}

template <typename Value>
bool ReadTaskValue(mach_port_t task, uint64_t address, Value* value) {
  mach_vm_size_t copied = 0;
  return mach_vm_read_overwrite(task, address, sizeof(Value),
                                reinterpret_cast<mach_vm_address_t>(value),
                                &copied) == KERN_SUCCESS &&
         copied == sizeof(Value);
}

inline std::string ReadTaskString(mach_port_t task, uint64_t address) {
  std::string value;
  for (size_t offset = 0; offset < 4096; offset += 256) {
    char chunk[256]{};
    mach_vm_size_t copied = 0;
    if (mach_vm_read_overwrite(task, address + offset, sizeof(chunk),
                               reinterpret_cast<mach_vm_address_t>(chunk),
                               &copied) != KERN_SUCCESS || copied == 0) {
      break;
    }
    const size_t length = strnlen(chunk, copied);
    value.append(chunk, length);
    if (length != copied) break;
  }
  return value;
}

inline std::vector<ImageSegment> DarwinRemoteImageSegments(mach_port_t task) {
  std::vector<ImageSegment> segments;
  task_dyld_info_data_t task_info_data{};
  mach_msg_type_number_t task_info_count = TASK_DYLD_INFO_COUNT;
  if (task_info(task, TASK_DYLD_INFO, reinterpret_cast<task_info_t>(&task_info_data),
                &task_info_count) != KERN_SUCCESS) {
    return segments;
  }
  dyld_all_image_infos all_images{};
  if (!ReadTaskValue(task, task_info_data.all_image_info_addr, &all_images) ||
      all_images.infoArray == nullptr || all_images.infoArrayCount > 65536) {
    return segments;
  }
  std::vector<dyld_image_info> images(all_images.infoArrayCount);
  mach_vm_size_t copied = 0;
  if (mach_vm_read_overwrite(task, reinterpret_cast<uint64_t>(all_images.infoArray),
                             images.size() * sizeof(dyld_image_info),
                             reinterpret_cast<mach_vm_address_t>(images.data()),
                             &copied) != KERN_SUCCESS ||
      copied != images.size() * sizeof(dyld_image_info)) {
    return segments;
  }
  for (const auto& image : images) {
    const uint64_t header_address = reinterpret_cast<uint64_t>(image.imageLoadAddress);
    mach_header_64 header{};
    if (!ReadTaskValue(task, header_address, &header) || header.magic != MH_MAGIC_64 ||
        header.sizeofcmds > 16 * 1024 * 1024) {
      continue;
    }
    std::vector<uint8_t> commands(header.sizeofcmds);
    if (mach_vm_read_overwrite(task, header_address + sizeof(header), commands.size(),
                               reinterpret_cast<mach_vm_address_t>(commands.data()),
                               &copied) != KERN_SUCCESS ||
        copied != commands.size()) {
      continue;
    }
    uint64_t text_vmaddr = 0;
    size_t command_offset = 0;
    for (uint32_t index = 0; index < header.ncmds && command_offset < commands.size(); ++index) {
      const auto* command = reinterpret_cast<const load_command*>(commands.data() + command_offset);
      if (command->cmdsize < sizeof(load_command) || command_offset + command->cmdsize > commands.size())
        break;
      if (command->cmd == LC_SEGMENT_64) {
        const auto* segment = reinterpret_cast<const segment_command_64*>(command);
        if (segment->fileoff == 0) text_vmaddr = segment->vmaddr;
      }
      command_offset += command->cmdsize;
    }
    const uint64_t slide = header_address - text_vmaddr;
    const std::string image_name =
        ReadTaskString(task, reinterpret_cast<uint64_t>(image.imageFilePath));
    command_offset = 0;
    for (uint32_t index = 0; index < header.ncmds && command_offset < commands.size(); ++index) {
      const auto* command = reinterpret_cast<const load_command*>(commands.data() + command_offset);
      if (command->cmdsize < sizeof(load_command) || command_offset + command->cmdsize > commands.size())
        break;
      if (command->cmd == LC_SEGMENT_64) {
        const auto* segment = reinterpret_cast<const segment_command_64*>(command);
        if (segment->vmsize != 0) {
          segments.push_back({segment->vmaddr + slide, segment->vmaddr + slide + segment->vmsize,
                              segment->fileoff, image_name});
        }
      }
      command_offset += command->cmdsize;
    }
  }
  return segments;
}

template <typename Callback>
bool ReadMapFile(const std::string& path, Callback callback) {
  pid_t process_id = getpid();
  unsigned parsed_id = 0;
  if (std::sscanf(path.c_str(), "/proc/%u/maps", &parsed_id) == 1) {
    process_id = static_cast<pid_t>(parsed_id);
  }
  mach_port_t task = mach_task_self();
  if (process_id != getpid()) {
    task = aim::remote_task::Acquire(process_id);
  }
  if (task == MACH_PORT_NULL) {
    return false;
  }
  const auto images =
      process_id == getpid() ? DarwinImageSegments() : DarwinRemoteImageSegments(task);
  mach_vm_address_t address = 0;
  natural_t depth = 0;
  mach_vm_address_t previous_address = 0;
  natural_t previous_depth = 0;
  while (true) {
    mach_vm_size_t size = 0;
    vm_region_submap_info_data_64_t info{};
    mach_msg_type_number_t count = VM_REGION_SUBMAP_INFO_COUNT_64;
    kern_return_t result = mach_vm_region_recurse(
        task, &address, &size, &depth,
        reinterpret_cast<vm_region_recurse_info_t>(&info), &count);
    if (result != KERN_SUCCESS) break;
    // Some task maps expose empty submap records while a child is blocked.
    // Never spin forever when the kernel returns no forward progress.
    if (size == 0 || (address == previous_address && depth == previous_depth)) break;
    previous_address = address;
    previous_depth = depth;
    if (info.is_submap) {
      ++depth;
      continue;
    }
    MapInfo map{address, address + size, 0, static_cast<uint64_t>(info.protection), ""};
    for (const auto& image : images) {
      if (address >= image.start && address < image.end) {
        map.pgoff = image.file_offset + (address - image.start);
        map.name = image.name;
        break;
      }
    }
    callback(map);
    if (address + size <= address) break;
    address += size;
  }
  if (process_id != getpid()) mach_port_deallocate(mach_task_self(), task);
  return true;
}

template <typename Callback>
bool ReadMapFileContent(char*, Callback) {
  return false;
}

}  // namespace android::procinfo

#endif

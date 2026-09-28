#include "darwin_audio_track.h"

extern "C" bool aim_android_aaudio_install_output_backend(
    DarwinAudioTrack* (*create)(int32_t, int32_t, int32_t, int32_t),
    void (*destroy)(DarwinAudioTrack*), void (*start)(DarwinAudioTrack*),
    void (*stop)(DarwinAudioTrack*),
    size_t (*write)(DarwinAudioTrack*, const void*, size_t, bool));

__attribute__((constructor)) static void InstallAAudioOutputBackend() {
  aim_android_aaudio_install_output_backend(
      &darwin_audio_track_create, &darwin_audio_track_destroy,
      &darwin_audio_track_start, &darwin_audio_track_stop,
      &darwin_audio_track_write);
}

#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#include <CoreAudio/CoreAudio.h>

#include <algorithm>
#include <atomic>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <iostream>
#include <limits>
#include <mutex>
#include <new>
#include <memory>
#include <vector>

struct DarwinAudioTrack {
  AudioUnit output = nullptr;
  std::mutex mutex;
  std::condition_variable space_available;
  std::vector<uint8_t> ring;
  size_t read_offset = 0;
  size_t write_offset = 0;
  size_t queued_bytes = 0;
  uint32_t bytes_per_frame = 4;
  uint32_t sample_rate = 48000;
  int32_t audio_format = 2;
  std::atomic<uint64_t> frames_consumed{0};
  std::atomic<bool> first_write_reported{false};
  std::atomic<bool> first_non_silent_write_reported{false};
  std::atomic<bool> first_render_reported{false};
  std::atomic<float> left_volume{1.0f};
  std::atomic<float> right_volume{1.0f};
  std::atomic<bool> released{false};
  bool started = false;
};

namespace {

uint32_t ChannelCount(int32_t mask) {
  const uint32_t count = static_cast<uint32_t>(__builtin_popcount(
      static_cast<uint32_t>(mask)));
  return std::clamp<uint32_t>(count, 1, 8);
}

void ApplyVolume(DarwinAudioTrack* track, void* bytes, size_t byte_count) {
  const float left = track->left_volume.load(std::memory_order_relaxed);
  const float right = track->right_volume.load(std::memory_order_relaxed);
  if (left == 1.0f && right == 1.0f) return;
  const uint32_t bytes_per_sample = track->audio_format == 4 ? 4 : 2;
  const uint32_t channels = track->bytes_per_frame / bytes_per_sample;
  if (channels == 0 || byte_count % bytes_per_sample != 0) return;
  const size_t sample_count = byte_count / bytes_per_sample;
  if (track->audio_format == 4) {
    auto* samples = static_cast<float*>(bytes);
    for (size_t index = 0; index < sample_count; ++index) {
      const float gain = index % channels == 0 ? left : right;
      samples[index] *= gain;
    }
  } else {
    auto* samples = static_cast<int16_t*>(bytes);
    for (size_t index = 0; index < sample_count; ++index) {
      const float gain = index % channels == 0 ? left : right;
      const int32_t scaled = static_cast<int32_t>(samples[index] * gain);
      samples[index] = static_cast<int16_t>(
          std::clamp(scaled, static_cast<int32_t>(INT16_MIN),
                     static_cast<int32_t>(INT16_MAX)));
    }
  }
}

OSStatus RenderAudio(void* context, AudioUnitRenderActionFlags* flags,
                     const AudioTimeStamp*, UInt32, UInt32,
                     AudioBufferList* buffers) {
  auto* track = static_cast<DarwinAudioTrack*>(context);
  if (track == nullptr || buffers == nullptr) return noErr;
  size_t consumed = 0;
  {
    std::lock_guard<std::mutex> lock(track->mutex);
    for (UInt32 index = 0; index < buffers->mNumberBuffers; ++index) {
      AudioBuffer& buffer = buffers->mBuffers[index];
      auto* destination = static_cast<uint8_t*>(buffer.mData);
      const size_t requested = buffer.mDataByteSize;
      size_t copied = 0;
      while (copied < requested && track->queued_bytes != 0) {
        const size_t contiguous = std::min(
            {requested - copied, track->queued_bytes,
             track->ring.size() - track->read_offset});
        std::memcpy(destination + copied,
                    track->ring.data() + track->read_offset, contiguous);
        track->read_offset = (track->read_offset + contiguous) % track->ring.size();
        track->queued_bytes -= contiguous;
        copied += contiguous;
      }
      if (copied < requested) {
        std::memset(destination + copied, 0, requested - copied);
      }
      if (copied != 0) {
        ApplyVolume(track, destination, copied);
        consumed += copied;
      }
    }
  }
  if (consumed != 0) {
    const uint64_t frames = consumed / track->bytes_per_frame;
    track->frames_consumed.fetch_add(frames, std::memory_order_relaxed);
    if (!track->first_render_reported.exchange(true,
                                                std::memory_order_relaxed)) {
      std::cerr << "ART Android AudioTrack: CoreAudio consumed frames="
                << frames << "\n";
    }
    track->space_available.notify_all();
  } else if (flags != nullptr) {
    *flags |= kAudioUnitRenderAction_OutputIsSilence;
  }
  return noErr;
}

void ReportAudioError(const char* operation, OSStatus status) {
  std::cerr << "ART Android AudioTrack: " << operation
            << " failed status=" << status << "\n";
}

}  // namespace

int32_t darwin_audio_primary_output_sample_rate(void) {
  AudioDeviceID device = kAudioObjectUnknown;
  UInt32 size = sizeof(device);
  AudioObjectPropertyAddress address{
      kAudioHardwarePropertyDefaultOutputDevice,
      kAudioObjectPropertyScopeGlobal,
      kAudioObjectPropertyElementMain,
  };
  if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, nullptr,
                                 &size, &device) != noErr ||
      device == kAudioObjectUnknown) {
    return 48000;
  }
  Float64 rate = 0.0;
  size = sizeof(rate);
  address = {
      kAudioDevicePropertyNominalSampleRate,
      kAudioObjectPropertyScopeGlobal,
      kAudioObjectPropertyElementMain,
  };
  if (AudioObjectGetPropertyData(device, &address, 0, nullptr, &size, &rate) !=
          noErr ||
      rate < 4000.0 || rate > 384000.0) {
    return 48000;
  }
  return static_cast<int32_t>(rate + 0.5);
}

int32_t darwin_audio_primary_output_frame_count(void) {
  AudioDeviceID device = kAudioObjectUnknown;
  UInt32 size = sizeof(device);
  AudioObjectPropertyAddress address{
      kAudioHardwarePropertyDefaultOutputDevice,
      kAudioObjectPropertyScopeGlobal,
      kAudioObjectPropertyElementMain,
  };
  if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, nullptr,
                                 &size, &device) != noErr ||
      device == kAudioObjectUnknown) {
    return 512;
  }
  UInt32 frames = 0;
  size = sizeof(frames);
  address = {
      kAudioDevicePropertyBufferFrameSize,
      kAudioObjectPropertyScopeGlobal,
      kAudioObjectPropertyElementMain,
  };
  if (AudioObjectGetPropertyData(device, &address, 0, nullptr, &size, &frames) !=
          noErr ||
      frames == 0 || frames > static_cast<UInt32>(INT32_MAX)) {
    return 512;
  }
  return static_cast<int32_t>(frames);
}

DarwinAudioTrack* darwin_audio_track_create(int32_t sample_rate,
                                            int32_t channel_mask,
                                            int32_t audio_format,
                                            int32_t buffer_bytes) {
  if (sample_rate <= 0 || buffer_bytes <= 0 ||
      (audio_format != 2 && audio_format != 4)) {
    return nullptr;
  }
  auto track = std::unique_ptr<DarwinAudioTrack>(
      new (std::nothrow) DarwinAudioTrack());
  if (track == nullptr) return nullptr;
  const uint32_t channels = ChannelCount(channel_mask);
  const uint32_t bytes_per_sample = audio_format == 4 ? 4 : 2;
  track->sample_rate = static_cast<uint32_t>(sample_rate);
  track->audio_format = audio_format;
  track->bytes_per_frame = channels * bytes_per_sample;
  const size_t one_second =
      static_cast<size_t>(sample_rate) * track->bytes_per_frame;
  const size_t requested = static_cast<size_t>(buffer_bytes) * 8u;
  try {
    track->ring.resize(std::max(one_second, requested));
  } catch (const std::bad_alloc&) {
    return nullptr;
  }

  AudioComponentDescription description{};
  description.componentType = kAudioUnitType_Output;
  description.componentSubType = kAudioUnitSubType_DefaultOutput;
  description.componentManufacturer = kAudioUnitManufacturer_Apple;
  AudioComponent component = AudioComponentFindNext(nullptr, &description);
  if (component == nullptr) return nullptr;
  OSStatus status = AudioComponentInstanceNew(component, &track->output);
  if (status != noErr) {
    ReportAudioError("AudioComponentInstanceNew", status);
    return nullptr;
  }

  AudioStreamBasicDescription stream{};
  stream.mSampleRate = sample_rate;
  stream.mFormatID = kAudioFormatLinearPCM;
  stream.mFormatFlags = kAudioFormatFlagIsPacked |
                        (audio_format == 4 ? kAudioFormatFlagIsFloat
                                           : kAudioFormatFlagIsSignedInteger);
  stream.mBytesPerPacket = track->bytes_per_frame;
  stream.mFramesPerPacket = 1;
  stream.mBytesPerFrame = track->bytes_per_frame;
  stream.mChannelsPerFrame = channels;
  stream.mBitsPerChannel = bytes_per_sample * 8;
  status = AudioUnitSetProperty(track->output, kAudioUnitProperty_StreamFormat,
                                kAudioUnitScope_Input, 0, &stream,
                                sizeof(stream));
  if (status == noErr) {
    AURenderCallbackStruct callback{};
    callback.inputProc = &RenderAudio;
    callback.inputProcRefCon = track.get();
    status = AudioUnitSetProperty(track->output,
                                  kAudioUnitProperty_SetRenderCallback,
                                  kAudioUnitScope_Input, 0, &callback,
                                  sizeof(callback));
  }
  if (status == noErr) status = AudioUnitInitialize(track->output);
  if (status != noErr) {
    ReportAudioError("configure output", status);
    AudioComponentInstanceDispose(track->output);
    track->output = nullptr;
    return nullptr;
  }
  std::cerr << "ART Android AudioTrack: CoreAudio ready rate=" << sample_rate
            << " channels=" << channels << " format=" << audio_format
            << " ring=" << track->ring.size() << "\n";
  return track.release();
}

void darwin_audio_track_destroy(DarwinAudioTrack* track) {
  if (track == nullptr) return;
  track->released.store(true, std::memory_order_release);
  track->space_available.notify_all();
  if (track->output != nullptr) {
    AudioOutputUnitStop(track->output);
    AudioUnitUninitialize(track->output);
    AudioComponentInstanceDispose(track->output);
  }
  delete track;
}

void darwin_audio_track_start(DarwinAudioTrack* track) {
  if (track == nullptr || track->output == nullptr) return;
  {
    std::lock_guard<std::mutex> lock(track->mutex);
    if (track->started) return;
  }
  const OSStatus status = AudioOutputUnitStart(track->output);
  if (status == noErr) {
    std::lock_guard<std::mutex> lock(track->mutex);
    track->started = true;
    std::cerr << "ART Android AudioTrack: CoreAudio started\n";
  } else {
    ReportAudioError("AudioOutputUnitStart", status);
  }
}

void darwin_audio_track_stop(DarwinAudioTrack* track) {
  if (track == nullptr || track->output == nullptr) return;
  AudioOutputUnitStop(track->output);
  std::lock_guard<std::mutex> lock(track->mutex);
  track->started = false;
}

void darwin_audio_track_pause(DarwinAudioTrack* track) {
  darwin_audio_track_stop(track);
}

void darwin_audio_track_flush(DarwinAudioTrack* track) {
  if (track == nullptr) return;
  std::lock_guard<std::mutex> lock(track->mutex);
  track->read_offset = 0;
  track->write_offset = 0;
  track->queued_bytes = 0;
  track->space_available.notify_all();
}

void darwin_audio_track_set_volume(DarwinAudioTrack* track, float left,
                                   float right) {
  if (track == nullptr) return;
  track->left_volume.store(std::clamp(left, 0.0f, 1.0f),
                           std::memory_order_relaxed);
  track->right_volume.store(std::clamp(right, 0.0f, 1.0f),
                            std::memory_order_relaxed);
}

int32_t darwin_audio_track_buffer_capacity_frames(DarwinAudioTrack* track) {
  if (track == nullptr || track->bytes_per_frame == 0) return 0;
  return static_cast<int32_t>(std::min<size_t>(
      track->ring.size() / track->bytes_per_frame, INT32_MAX));
}

uint32_t darwin_audio_track_position(DarwinAudioTrack* track) {
  return track == nullptr
             ? 0
             : static_cast<uint32_t>(track->frames_consumed.load(
                   std::memory_order_relaxed));
}

size_t darwin_audio_track_write(DarwinAudioTrack* track, const void* data,
                                size_t size, bool blocking) {
  if (track == nullptr || data == nullptr || size == 0) return 0;
  const auto* source = static_cast<const uint8_t*>(data);
  const bool has_signal =
      std::any_of(source, source + size, [](uint8_t sample) { return sample != 0; });
  size_t written = 0;
  std::unique_lock<std::mutex> lock(track->mutex);
  while (written < size &&
         !track->released.load(std::memory_order_acquire)) {
    const size_t available = track->ring.size() - track->queued_bytes;
    if (available == 0) {
      if (!blocking) break;
      track->space_available.wait(lock);
      continue;
    }
    const size_t contiguous = std::min(
        {size - written, available, track->ring.size() - track->write_offset});
    std::memcpy(track->ring.data() + track->write_offset, source + written,
                contiguous);
    track->write_offset = (track->write_offset + contiguous) % track->ring.size();
    track->queued_bytes += contiguous;
    written += contiguous;
  }
  if (written != 0 &&
      !track->first_write_reported.exchange(true, std::memory_order_relaxed)) {
    std::cerr << "ART Android AudioTrack: first PCM write bytes=" << written
              << "\n";
  }
  if (written != 0 && has_signal &&
      !track->first_non_silent_write_reported.exchange(
          true, std::memory_order_relaxed)) {
    std::cerr << "ART Android AudioTrack: first non-silent PCM write bytes="
              << written << "\n";
  }
  return written;
}

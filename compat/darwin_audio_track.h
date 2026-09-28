#pragma once

#include <cstddef>
#include <cstdint>

struct DarwinAudioTrack;

#ifdef __cplusplus
extern "C" {
#endif
DarwinAudioTrack* darwin_audio_track_create(int32_t sample_rate,
                                            int32_t channel_mask,
                                            int32_t audio_format,
                                            int32_t buffer_bytes);
int32_t darwin_audio_primary_output_sample_rate(void);
int32_t darwin_audio_primary_output_frame_count(void);
void darwin_audio_track_destroy(DarwinAudioTrack* track);
void darwin_audio_track_start(DarwinAudioTrack* track);
void darwin_audio_track_stop(DarwinAudioTrack* track);
void darwin_audio_track_pause(DarwinAudioTrack* track);
void darwin_audio_track_flush(DarwinAudioTrack* track);
void darwin_audio_track_set_volume(DarwinAudioTrack* track, float left,
                                   float right);
int32_t darwin_audio_track_buffer_capacity_frames(DarwinAudioTrack* track);
uint32_t darwin_audio_track_position(DarwinAudioTrack* track);
size_t darwin_audio_track_write(DarwinAudioTrack* track, const void* data,
                                size_t size, bool blocking);
#ifdef __cplusplus
}
#endif

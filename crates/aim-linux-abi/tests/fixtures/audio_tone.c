// Plays a tone and records through the original libaaudio, AudioFlinger
// and the audio HAL (tests/audio.rs). AAudio takes its legacy path
// (AudioTrack / AudioRecord), since the HAL has no MMAP ports.
//
// Prints one "key value" line per measurement and "ok done" at the end.
// Usage: audio_tone [play_ms] [record_ms]

#include <aaudio/AAudio.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#define RATE 48000
#define TONE_HZ 440.0
// -90 dBFS: inaudible; the HAL's peak meter still sees it.
#define AMPLITUDE 3.162e-5f

static int64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000 + ts.tv_nsec;
}

static AAudioStream *open_stream(aaudio_direction_t direction, int channels) {
    AAudioStreamBuilder *builder;
    AAudioStream *stream = NULL;
    if (AAudio_createStreamBuilder(&builder) != AAUDIO_OK) return NULL;
    AAudioStreamBuilder_setDirection(builder, direction);
    AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_FLOAT);
    AAudioStreamBuilder_setChannelCount(builder, channels);
    AAudioStreamBuilder_setSampleRate(builder, RATE);
    aaudio_result_t r = AAudioStreamBuilder_openStream(builder, &stream);
    AAudioStreamBuilder_delete(builder);
    if (r != AAUDIO_OK) {
        printf("open %s: %s\n", direction == AAUDIO_DIRECTION_OUTPUT ? "output" : "input",
               AAudio_convertResultToText(r));
        return NULL;
    }
    printf("%s_stream rate %d channels %d burst %d capacity %d mode %d\n",
           direction == AAUDIO_DIRECTION_OUTPUT ? "output" : "input",
           AAudioStream_getSampleRate(stream), AAudioStream_getChannelCount(stream),
           AAudioStream_getFramesPerBurst(stream), AAudioStream_getBufferCapacityInFrames(stream),
           AAudioStream_getPerformanceMode(stream));
    return stream;
}

static int play(int ms) {
    AAudioStream *s = open_stream(AAUDIO_DIRECTION_OUTPUT, 2);
    if (!s) return 1;
    if (AAudioStream_requestStart(s) != AAUDIO_OK) return 1;
    enum { CHUNK = 480 };
    float buf[CHUNK * 2];
    int64_t phase = 0, total = (int64_t)RATE * ms / 1000;
    double latency_sum = 0;
    int latency_n = 0;
    while (phase < total) {
        for (int i = 0; i < CHUNK; i++, phase++) {
            float v = AMPLITUDE * sinf((float)(2.0 * M_PI * TONE_HZ * phase / RATE));
            buf[2 * i] = buf[2 * i + 1] = v;
        }
        aaudio_result_t n = AAudioStream_write(s, buf, CHUNK, 1000000000LL);
        if (n < 0) {
            printf("write: %s\n", AAudio_convertResultToText(n));
            return 1;
        }
        // When would the frame just written be heard? From the presented
        // position and its time.
        int64_t pos, t;
        if (phase > RATE / 2 &&
            AAudioStream_getTimestamp(s, CLOCK_MONOTONIC, &pos, &t) == AAUDIO_OK) {
            int64_t written = AAudioStream_getFramesWritten(s);
            double heard = t + (written - pos) * 1e9 / RATE;
            latency_sum += (heard - now_ns()) / 1e6;
            latency_n++;
        }
    }
    int64_t pos = -1, t = -1;
    aaudio_result_t ts = AAudioStream_getTimestamp(s, CLOCK_MONOTONIC, &pos, &t);
    printf("output_frames_written %lld\n", (long long)AAudioStream_getFramesWritten(s));
    printf("output_frames_read %lld\n", (long long)AAudioStream_getFramesRead(s));
    printf("output_timestamp %s position %lld age_ms %.1f\n", AAudio_convertResultToText(ts),
           (long long)pos, (now_ns() - t) / 1e6);
    printf("output_xruns %d\n", AAudioStream_getXRunCount(s));
    if (latency_n) printf("output_latency_ms %.1f\n", latency_sum / latency_n);
    AAudioStream_requestStop(s);
    AAudioStream_close(s);
    return 0;
}

static int record(int ms) {
    AAudioStream *s = open_stream(AAUDIO_DIRECTION_INPUT, 1);
    if (!s) return 1;
    if (AAudioStream_requestStart(s) != AAUDIO_OK) return 1;
    enum { CHUNK = 480 };
    float buf[CHUNK];
    int64_t got = 0, want = (int64_t)RATE * ms / 1000;
    float peak = 0;
    int64_t start = now_ns();
    while (got < want) {
        aaudio_result_t n = AAudioStream_read(s, buf, CHUNK, 1000000000LL);
        if (n < 0) {
            printf("read: %s\n", AAudio_convertResultToText(n));
            return 1;
        }
        for (int i = 0; i < n; i++) peak = fmaxf(peak, fabsf(buf[i]));
        got += n;
    }
    printf("input_frames %lld in_ms %.0f peak %.5f\n", (long long)got, (now_ns() - start) / 1e6,
           peak);
    AAudioStream_requestStop(s);
    AAudioStream_close(s);
    return 0;
}

int main(int argc, char **argv) {
    int play_ms = argc > 1 ? atoi(argv[1]) : 2000;
    int record_ms = argc > 2 ? atoi(argv[2]) : 1000;
    setvbuf(stdout, NULL, _IOLBF, 0);
    if (play_ms > 0 && play(play_ms)) return 1;
    if (record_ms > 0 && record(record_ms)) return 1;
    printf("ok done\n");
    return 0;
}

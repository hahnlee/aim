#include "tracing_perfetto.h"
#include "trace_categories.h"
#include <perfetto/public/abi/tracing_session_abi.h>
#include <perfetto/public/abi/heap_buffer.h>
#include <perfetto/public/protos/config/trace_config.pzc.h>
#include <perfetto/public/protos/config/data_source_config.pzc.h>
#include <perfetto/public/protos/config/track_event/track_event_config.pzc.h>
#include <cassert>
#include <cstdio>
#include <string>
#include <vector>

int main() {
  tracing_perfetto::registerWithPerfetto(true); // Original in-process test backend.
  assert(!tracing_perfetto::isTagEnabled(TRACE_CATEGORY_APP));
  PerfettoPbMsgWriter writer{};
  auto* heap = PerfettoHeapBufferCreate(&writer.writer);
  perfetto_protos_TraceConfig config{};
  PerfettoPbMsgInit(&config.msg, &writer);
  perfetto_protos_TraceConfig_BufferConfig buffer{};
  perfetto_protos_TraceConfig_begin_buffers(&config, &buffer);
  perfetto_protos_TraceConfig_BufferConfig_set_size_kb(&buffer, 1024);
  perfetto_protos_TraceConfig_end_buffers(&config, &buffer);
  perfetto_protos_TraceConfig_DataSource source{};
  perfetto_protos_TraceConfig_begin_data_sources(&config, &source);
  perfetto_protos_DataSourceConfig data{};
  perfetto_protos_TraceConfig_DataSource_begin_config(&source, &data);
  perfetto_protos_DataSourceConfig_set_cstr_name(&data, "track_event");
  perfetto_protos_TrackEventConfig categories{};
  perfetto_protos_DataSourceConfig_begin_track_event_config(&data, &categories);
  perfetto_protos_TrackEventConfig_set_enabled_categories(&categories, "app", 3);
  perfetto_protos_DataSourceConfig_end_track_event_config(&data, &categories);
  perfetto_protos_TraceConfig_DataSource_end_config(&source, &data);
  perfetto_protos_TraceConfig_end_data_sources(&config, &source);
  size_t size = PerfettoStreamWriterGetWrittenSize(&writer.writer);
  std::vector<char> bytes(size);
  PerfettoHeapBufferCopyInto(heap, &writer.writer, bytes.data(), size);
  PerfettoHeapBufferDestroy(heap, &writer.writer);
  auto* session = PerfettoTracingSessionInProcessCreate();
  assert(session != nullptr);
  PerfettoTracingSessionSetup(session, bytes.data(), bytes.size());
  PerfettoTracingSessionStartBlocking(session);
  assert(tracing_perfetto::isTagEnabled(TRACE_CATEGORY_APP));
  tracing_perfetto::traceBegin(TRACE_CATEGORY_APP, "darwin-trace-begin");
  tracing_perfetto::traceEnd(TRACE_CATEGORY_APP);
  tracing_perfetto::traceInstant(TRACE_CATEGORY_APP, "darwin-trace-instant");
  tracing_perfetto::traceCounter(TRACE_CATEGORY_APP, "darwin-trace-counter", 42);
  assert(PerfettoTracingSessionFlushBlocking(session, 5000));
  PerfettoTracingSessionStopBlocking(session);
  std::string trace;
  PerfettoTracingSessionReadTraceBlocking(session,
      [](PerfettoTracingSessionImpl*, const void* data, size_t size, bool, void* output) {
        static_cast<std::string*>(output)->append(static_cast<const char*>(data), size);
      }, &trace);
  PerfettoTracingSessionDestroy(session);
  for (const char* marker : {"darwin-trace-begin", "darwin-trace-instant", "darwin-trace-counter"})
    assert(trace.find(marker) != std::string::npos);
  assert(!tracing_perfetto::isTagEnabled(TRACE_CATEGORY_APP));
  std::printf("AOSP Trace + Perfetto: enable/record/flush/read/disable PASS bytes=%zu\n", trace.size());
}

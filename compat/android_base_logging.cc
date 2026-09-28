#include "android-base/logging.h"

#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <string>

namespace android::base {

class LogMessageData {
 public:
  LogMessageData(const char* file, unsigned int line, LogSeverity severity, const char* tag,
                 int error)
      : file(file), line(line), severity(severity), tag(tag), error(error) {}

  const char* file;
  unsigned int line;
  LogSeverity severity;
  const char* tag;
  int error;
  std::ostringstream stream;
};

namespace {

std::atomic<LogSeverity> minimum_severity{INFO};
LogFunction logger{StderrLogger};

const char* SeverityName(LogSeverity severity) {
  switch (severity) {
    case VERBOSE: return "VERBOSE";
    case DEBUG: return "DEBUG";
    case INFO: return "INFO";
    case WARNING: return "WARNING";
    case ERROR: return "ERROR";
    case FATAL_WITHOUT_ABORT: return "FATAL";
    case FATAL: return "FATAL";
  }
}

}  // namespace

LogMessage::LogMessage(const char* file, unsigned int line, LogId, LogSeverity severity,
                       const char* tag, int error)
    : LogMessage(file, line, severity, tag, error) {}

LogMessage::LogMessage(const char* file, unsigned int line, LogSeverity severity, const char* tag,
                       int error)
    : data_(new LogMessageData(file, line, severity, tag, error)) {}

LogMessage::~LogMessage() {
  std::string message = data_->stream.str();
  if (data_->error != -1) {
    message.append(": ");
    message.append(std::strerror(data_->error));
  }
  LogLine(data_->file, data_->line, data_->severity, data_->tag, message.c_str());
  if (data_->severity == FATAL) {
    std::abort();
  }
}

std::ostream& LogMessage::stream() {
  return data_->stream;
}

void LogMessage::LogLine(const char* file, unsigned int line, LogSeverity severity, const char* tag,
                         const char* message) {
  logger(DEFAULT, severity, tag, file, line, message);
}

void StderrLogger(LogId, LogSeverity severity, const char* tag, const char* file, unsigned int line,
                  const char* message) {
  std::fprintf(stderr, "%s %s:%u%s%s: %s\n", SeverityName(severity), file, line,
               tag == nullptr ? "" : " ", tag == nullptr ? "" : tag, message);
}

LogFunction SetLogger(LogFunction&& replacement) {
  LogFunction previous = std::move(logger);
  logger = std::move(replacement);
  return previous;
}

LogSeverity GetMinimumLogSeverity() {
  return minimum_severity.load(std::memory_order_relaxed);
}

LogSeverity SetMinimumLogSeverity(LogSeverity severity) {
  return minimum_severity.exchange(severity, std::memory_order_relaxed);
}

bool ShouldLog(LogSeverity severity, const char*) {
  return severity >= GetMinimumLogSeverity();
}

ScopedLogSeverity::ScopedLogSeverity(LogSeverity severity) : old_(SetMinimumLogSeverity(severity)) {}

ScopedLogSeverity::~ScopedLogSeverity() {
  SetMinimumLogSeverity(old_);
}

}  // namespace android::base

// Test-data converter only. All parsing/encoding is protobuf's implementation
// generated from the original AOSP schemas; never used by APK startup.
#include "apex_manifest.pb.h"
#include "linker_config.pb.h"
#include <google/protobuf/util/json_util.h>
#include <fstream>
#include <iostream>
#include <iterator>
#include <memory>

int main(int argc, char** argv) {
  if (argc != 4) return 2;
  std::unique_ptr<google::protobuf::Message> message;
  if (std::string(argv[1]) == "apex") message = std::make_unique<apex::proto::ApexManifest>();
  else if (std::string(argv[1]) == "linker") message = std::make_unique<android::linkerconfig::proto::LinkerConfig>();
  else return 2;
  std::ifstream input(argv[2]);
  if (!input) return 1;
  const std::string json{std::istreambuf_iterator<char>(input), {}};
  const auto status = google::protobuf::util::JsonStringToMessage(json, message.get());
  if (!status.ok()) { std::cerr << status << '\n'; return 1; }
  std::ofstream output(argv[3], std::ios::binary);
  return output && message->SerializeToOstream(&output) ? 0 : 1;
}

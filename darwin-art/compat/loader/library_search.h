#pragma once

#include <string>

namespace darwin_art::loader {

// Platform file lookup only. The namespace owner supplies the authorized
// ordered search path. This does not authorize absolute paths or dependencies,
// and must not add environment, current-directory or process-global searches.
std::string FindLibraryInSearchPath(const std::string& soname,
                                    const std::string& search_path);

}  // namespace darwin_art::loader

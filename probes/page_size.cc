#include <iostream>

#include "base/globals.h"

int main() {
  std::cout << "ART Darwin page size: " << art::GetPageSizeSlow() << '\n';
  return 0;
}

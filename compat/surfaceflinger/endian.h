#pragma once

#include <libkern/OSByteOrder.h>

#define htobe16(value) OSSwapHostToBigInt16(value)
#define htole16(value) OSSwapHostToLittleInt16(value)
#define be16toh(value) OSSwapBigToHostInt16(value)
#define le16toh(value) OSSwapLittleToHostInt16(value)
#define htobe32(value) OSSwapHostToBigInt32(value)
#define htole32(value) OSSwapHostToLittleInt32(value)
#define be32toh(value) OSSwapBigToHostInt32(value)
#define le32toh(value) OSSwapLittleToHostInt32(value)
#define htobe64(value) OSSwapHostToBigInt64(value)
#define htole64(value) OSSwapHostToLittleInt64(value)
#define be64toh(value) OSSwapBigToHostInt64(value)
#define le64toh(value) OSSwapLittleToHostInt64(value)

#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

extern "C" {
#include "jpeglib.h"
}
#include "ultrahdr_api.h"

static uint64_t fnv1a(const unsigned char* data, size_t size) {
  uint64_t hash = 1469598103934665603ULL;
  for (size_t i = 0; i < size; ++i) {
    hash = (hash ^ data[i]) * 1099511628211ULL;
  }
  return hash;
}

int main() {
  constexpr int width = 4;
  constexpr int height = 4;
  unsigned char rgb[width * height * 3];
  for (int y = 0; y < height; ++y) {
    for (int x = 0; x < width; ++x) {
      const int i = (y * width + x) * 3;
      rgb[i] = static_cast<unsigned char>(x * 61);
      rgb[i + 1] = static_cast<unsigned char>(y * 59);
      rgb[i + 2] = static_cast<unsigned char>((x + y) * 29);
    }
  }

  jpeg_compress_struct encoder{};
  jpeg_error_mgr encoder_error{};
  encoder.err = jpeg_std_error(&encoder_error);
  jpeg_create_compress(&encoder);
  unsigned char* encoded = nullptr;
  unsigned long encoded_size = 0;
  jpeg_mem_dest(&encoder, &encoded, &encoded_size);
  encoder.image_width = width;
  encoder.image_height = height;
  encoder.input_components = 3;
  encoder.in_color_space = JCS_RGB;
  jpeg_set_defaults(&encoder);
  jpeg_set_quality(&encoder, 90, TRUE);
  jpeg_start_compress(&encoder, TRUE);
  while (encoder.next_scanline < encoder.image_height) {
    JSAMPROW row = rgb + encoder.next_scanline * width * 3;
    jpeg_write_scanlines(&encoder, &row, 1);
  }
  jpeg_finish_compress(&encoder);
  jpeg_destroy_compress(&encoder);
  if (encoded_size < 4 || encoded[0] != 0xff || encoded[1] != 0xd8) return 10;

  if (is_uhdr_image(encoded, static_cast<int>(encoded_size)) != 0) return 11;
  uhdr_codec_private_t* decoder = uhdr_create_decoder();
  if (decoder == nullptr) return 12;
  uhdr_release_decoder(decoder);

  jpeg_decompress_struct decoder_jpeg{};
  jpeg_error_mgr decoder_error{};
  decoder_jpeg.err = jpeg_std_error(&decoder_error);
  jpeg_create_decompress(&decoder_jpeg);
  jpeg_mem_src(&decoder_jpeg, encoded, encoded_size);
  if (jpeg_read_header(&decoder_jpeg, TRUE) != JPEG_HEADER_OK) return 13;
  jpeg_start_decompress(&decoder_jpeg);
  if (decoder_jpeg.output_width != width || decoder_jpeg.output_height != height) return 14;
  std::vector<unsigned char> decoded(width * height * decoder_jpeg.output_components);
  while (decoder_jpeg.output_scanline < decoder_jpeg.output_height) {
    JSAMPROW row = decoded.data() + decoder_jpeg.output_scanline * width * decoder_jpeg.output_components;
    jpeg_read_scanlines(&decoder_jpeg, &row, 1);
  }
  jpeg_finish_decompress(&decoder_jpeg);
  jpeg_destroy_decompress(&decoder_jpeg);
  const auto hash = fnv1a(encoded, encoded_size);
  std::free(encoded);
  std::printf("codec-smoke: jpeg=%lu fnv1a=%016llx ultrahdr-regular-jpeg=0\n",
              encoded_size, static_cast<unsigned long long>(hash));
  return 0;
}

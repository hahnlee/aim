# Darwin MediaCodec bridge

The runtime now exposes a host-backed Android codec instead of an empty
`MediaCodecList`. `c2.aim.avc.decoder` and `c2.aim.hevc.decoder` are
advertised for `video/avc` and `video/hevc`; their Java `MediaCodec` paths are
backed by VideoToolbox and accept the Android CSD configuration records.
Decoded frames are
returned as Android flexible YUV 4:2:0 (`ByteBuffer` output), and a configured
`Surface` is published through the existing Darwin `ANativeWindow` bridge.

The registry intentionally does not claim AAC, Opus, or encoder support until
each has a matching host implementation. Applications therefore see accurate
capability discovery and can select their own software fallback for those MIME
types. The NDK media ABI uses the same backend for AVC/HEVC decoder creation,
`AMediaFormat_setBuffer` CSD records, queue/dequeue, and output-buffer
ownership. Encoder and encrypted-input APIs remain unsupported.

The native implementation lives in `compat/darwin_media_codec.cc`, separate
from the general framework JNI translation unit. CoreMedia, CoreVideo, and
VideoToolbox are linked only by the runtime flavors that include this adapter.

# Movies

Video decoding for games: the SDK's movie library (`movie::Decoder`, Switch 1 and 2), the Switch 2 SDK's
nn::codec video decoders, and the pictures they hand to NVN. On a console these wrap NVIDIA's hardware decoder over
nvdrv; the host has none, so the SDK's calls are taken over by name and decoded on the CPU.

| Part | File |
|---|---|
| movie::Decoder takeover | `src/os/src/services/movie.rs` |
| Switch 2 video decoders and sync points | `src/os/src/services/codec_video.rs` |
| VP8 / VP9 (libvpx 1.14.0, decoders only) | `src/os/src/media/vpx.rs` |
| AV1 (dav1d 1.5.5) | `src/os/src/media/av1.rs` |
| H.264 and AAC: Media Foundation (Windows), FFmpeg (Linux) | `src/os/src/media/mf.rs`, `src/os/src/media/ff.rs` |
| Vorbis | the `lewton` crate, from `movie.rs` |
| Pictures and colour conversion | `src/os/src/media/yuv.rs` |
| Native images as NVN textures | `src/graphics/src/nvn/native.rs` |

## Decoder lifecycle

The protocol the SDK's Decoder has (Android MediaCodec-like):

1. The client registers four nn::os events: input available, output ready, format changed, error.
2. Inputs: AcquireInputBufferIndex, GetInputBuffer, SendInputBufferForDecode(index, offset, size, time in µs, flags).
3. Outputs: AcquireOutputBufferIndex, GetOutputBufferFormat / GetOutputBuffer, ReleaseOutputBufferIndex.

Packets queue in arrival order. Whenever an output slot is free the head packet is decoded and its input index
returned; while the client holds every slot, packets stay queued (back-pressure, as MediaCodec). Each decoder decodes
on a host thread of its own (`worker`); video results are signalled by a guest thread the host owns, since
SignalEvent is guest code (`signaller`); audio results by the caller. A reset or close (`reset`) drops queued work.

## Output slots

- **CPU mode:** NV12 into the client's buffer, luma stride equal to the width.
- **Native-texture mode:** RGBA into the client's NVN texture, written through graphics' own procs
  (`nvnTextureGet{Width,Height,Format}`, `nvnTextureWriteTexelsStrided`); the buffer's sync is marked done by
  `nvnSyncInitialize` (an unfenced sync is signalled in graphics).
- Colour: the stream's declared matrix and range; undeclared streams use BT.709 from 720 lines up, else BT.601
  (`yuv.rs`).

`NXOS_MOVIE_DUMP=<dir>` writes the first pictures handed to the game as PNG, to separate a decode problem from a
drawing one.

## The Switch 2 decoder API

`codec_video.rs` (nn::codec video decoders: VP9, H.264, AV1, VP8):

- The client makes one image pool per picture (MakeDecodeImagePreference, CalculateDecoderImagePoolBuffer,
  InitializeDecoderImagePool) and registers each under an index; registering makes it acquirable (a queue 8 deep)
  with a sync handle already signalled.
- Acquire takes the oldest index (3604 when none); Release gives the buffer to the decoder and makes it acquirable
  again at once, with a new sync handle that signals when a picture is in it. The client polls
  `nn::gs::TryWaitNativeSync` or waits on an nn::syncpt event, then reads properties (3604 until filled) and the
  picture (ReadDecoderOutputImageRgba / Nv12), or draws it through a native image.
- Submit takes one packet with its time and an end-of-stream flag; 3603 (busy) when waiting packets and pictures fill
  `CV_HOLD`, the submission-ready event going down at `CV_READY`.

### Native images

`nn::gs::NativeImageHandle` (slot << 8 | picture) names a picture of a decoder's image pool, kept as RGBA in memory
(`codec_video::native_image`). `nvnTextureInitializeFromNativeImage` makes a linear texture over a memory pool of that
memory, so each new picture is read with no host copy (`src/graphics/src/nvn/native.rs`). These entry points are not
in the NVN proc table: the game imports them by name, so the OS layer binds them (`takeovers()`).

## Demuxers and codecs

- The SDK's extractor and the engines' own demuxers run as the game's code; only decoding is the host's. (PORT_PLAN
  5A.2 lists a `movie::Extractor::SetDataSource` takeover as done; no such takeover is in `src/os/src/` today.)
- H.264 arrives as AVCC from the SDK and is converted to Annex B (`avcc_annexb`); Media Foundation reorders to
  presentation order and crops to the display size.
- AV1 follows the owner's dav1d driving rules (`av1.rs`): frame threads, one `dav1d_get_picture` per
  `dav1d_send_data`, a drain runs to EAGAIN before the next unit, 10 / 12-bit and 4:2:2 / 4:4:4 brought to 8-bit 4:2:0.
- AAC through Media Foundation's decoder (`aac_shape` reads the stream's channel layout); Vorbis through lewton;
  audio comes out as PCM16.
- The codec archives are MinGW-built static libraries in `src/os/third_party/` (Windows); `src/os/src/media/crt.rs`
  supplies the C runtime pieces they expect. On Linux the system libraries are linked instead (`src/os/build.rs`).

`nvdrv` itself answers the SDK's hardware path with NvError 3 so the decoder takes its own failure path
(`src/os/src/services/nvdrv.rs`); the takeovers above stop games from reaching it.

## Movie players

### MK8D's menu preview panel

- **Symptom:** Mario Kart 8 Deluxe's mode-select preview panel draws black although the H.264 pictures decode fine.
- **Cause:** `nvnTextureWriteTexels` wrote only the one texture object's image; the menu samples its own texture
  object over the same storage.
- **Fix:** the texels are written into the storage, swizzled, and every image over that storage uploads from it again
  (INTERFACES lane G3; GPU test `write_texels_reach_every_texture_over_the_storage`).

### Ocarina of Time's movies and nn::ndve

Ocarina of Time decodes AV1 (dav1d) and runs nn::ndve's upscaling network as translated compute shaders. nxnative
replaces that network with host kernels; nxrs does not, which only costs speed. Tracked as PORT_PLAN 5A.2
("Movie / NDVE / NDSR network run by the host"), deferred with M8. The game itself does not yet reach its movies
(it stops at a static-init fault, PORT_PLAN M8).

## Known limits

- The SDK's hardware decoder path (nvdrv) is never served; every stream goes through a takeover above.
- nxnative's VideoToolbox path has no counterpart; Media Foundation is the Windows equivalent.

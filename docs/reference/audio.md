# Audio

The game's sound, from the SDK's audio renderer and AudioOut to the PC's sound card, plus capture (AudioIn) and the
hardware Opus decoder. Services are in `src/os/src/services/` (`audren.rs`, `audren_ipc.rs`, `audio.rs`,
`audout_ipc.rs`, `codec.rs`); the sound card and codecs in `src/os/src/media/` (`sink.rs`, `mic.rs`, `opus.rs`).
Movie audio is in [movies.md](movies.md).

## The sound card

`src/os/src/media/sink.rs`. Every stream the game produces (each renderer's output, each AudioOut, each SDK 21
OutputStream) is a queue of 48 kHz stereo blocks; one output device plays them mixed:

- The device is the system's default output through WASAPI (cpal), in its own format and rate, resampled linearly
  from 48 kHz. `NXOS_AUDIO_DEVICE=<part of a name>` picks another; a name no device has falls back to the default
  with a red line.
- A block goes back to its producer (released, with its tag) once a 48 kHz wall clock passes its end, smoothly every
  2 ms, not when the device callback reaches it. Producers paced by their releases therefore keep real time.
- A stream starts playing once it holds 50 ms or has waited 50 ms.
- The mix goes through a linked peak limiter (instant attack, 100 ms release).
- Muting (`mute_on_focus_loss`, or `NXOS_AUDIO_MUTE=1` for a run) fades the device to silence in 10 ms; the streams
  play on at their pace and recordings keep the sound.
- `set_tap` gives the mix before slowdown to the F7 recorder (`src/front/src/rec.rs`).

### Smooth slowdown

`audio_slowdown=1` (default; F1 toggles it): when a stream runs short because the PC cannot produce sound in real
time, it plays slower, eased toward its queue fill, keeping every queued sample. Off, a stream that runs dry is an
underrun: its last sample fades out and the gap is counted (`audio_underruns` metric).

### Log lines

| Line | Meaning |
|---|---|
| `audio: output on <device>; smooth slowdown on` | the device opened |
| `audio: <stream>: <n> s played (<m> s not silent), <u> underruns, slowed down <s> s` | per stream statistics |
| channel RMS / peak lines | every 5 s of output for the first 5 minutes, then every minute; silence once, then every minute |
| `[RED] audio: no output device (...): the game's sound is consumed in real time by a clock, not heard` | no device; the game keeps its pace |
| `audio: the output device <name> went away or changed: opening the default device again` | device change |

`NXOS_AUDIO_DUMP=<file.wav>` writes the mix as it goes out (48 kHz stereo 16-bit, header kept valid as it grows).

## Audio renderer

### SDKs with their own renderer (2.x and later)

The game's SDK ships the complete renderer (server side and the CPU version of DSP command processing); the host runs
it in process and adds no mixer of its own (`audren.rs`, nxnative's design):

- `GetAudioRendererWorkBufferSize` keeps the DSP service's budget; `OpenAudioRenderer` opens the SDK's renderer in
  "CPU rendering + manual execution" mode (parameter +0x2c = 1, +0x30 = 1). When the game's buffer is too small for
  the local renderer, the host backs the rest (`audren: the host backs the local renderer's work buffer ...`).
- A render thread (a guest thread the SDK creates, running the host's loop) renders one frame each time the sound card
  hands a 5 ms block back, so the sound card's clock paces the renderer.
- `ExecuteAudioRendererRendering` (and SDK 20's `AudioRenderSystem` one) is taken over: the SDK generates the command
  list under its update lock, its `CommandListProcessor` runs it on the CPU, then the host finds the DeviceSink
  command and folds its input mix buffers to stereo.
- `AudioRenderSystem::Update` renders a catch-up frame first when nothing was rendered since the last update (an
  effect's initialization must reach the DSP between two updates).

### Command lists by SDK

| SDK | Command header | DeviceSink |
|---|---|---|
| 4 / 6 | +4 size, +6 command ID, +0x10 enabled | 0x130 bytes, s8 inputs (SDK 4); 0x138 (SDK 5) |
| 8-20 | as SDK 6 | 0x148 bytes |
| 21 | no magic: +0 enabled, +1 command ID, +2 size | 0x140 bytes |

The layout chosen is logged once (`audren: command headers in the SDK ... layout`). 32 kHz renderers are upsampled
3:2 by the host, since the SDK makes no upsampler for CPU rendering.

### SDK 1.x: no renderer in the SDK

`audren_ipc.rs` serves `audren:u` itself (Turbo's design): every RequestUpdate parses voices, channel resources,
effects, splitters, mixes and sinks, mixes 5 ms blocks of 48 kHz stereo, and answers played samples and consumed
wave buffers. Voices: PCM 8 / 16 / 24 / 32 / float and DSP ADPCM, linear resampling by rate x pitch, two biquads, up
to four wave buffers; REV15's 0x188-byte voices are read as 0x170. Effects: buffer mixer, biquad, auxiliary sends.
Blocks go to the sound card at most 8 ahead (40 ms). Log: `audren: the SDK has no renderer of its own (SDK 1.x) ...`.

## AudioOut and devices

`audio.rs` (later SDKs, by name) and `audout_ipc.rs` (SDK 1.x over IPC) share one model:

- AudioOut: the game appends PCM16 buffers (`AudioOutBuffer`: data +0x08, buffer size +0x10, data size +0x18) and
  takes them back once played; the release event fires as each ends. The front of the guest object holds rate,
  channels, format and state, which middleware reads directly. 48 kHz only; six channels fold to stereo.
- SDK 21 OutputStream on the one virtual output device: StreamFormat (PCM16 or float, channels, rate) and a capacity
  in frames; each write is one block on a stream of its own; shortest stream 10 ms.
- AudioIn: the default capture device through WASAPI (`src/os/src/media/mic.rs`), resampled to 48 kHz PCM16. With no microphone
  the device list is empty and an open fails, as on a console without one. SDK 21's InputStream lists no device.
- Device names, volume and notifications are answered from the host device.

## Codecs

`codec.rs`: nn::codec's hardware Opus decoders (single and multi-stream, their C interface, SDK 21's asynchronous
manager, and the `hwopus` IPC service) decoded with libopus 1.5.2 (`src/os/src/media/opus.rs`). Packets carry the Switch's
8-byte header (size and final range, big-endian); PCM comes out interleaved 16-bit. The game's work buffer is not
used; the decoder state is kept by the guest object's address.

## Problems found on games

### Voices that reach no mix

- **Symptom:** Ocarina of Time's opening is silent.
- **Cause:** its voices name no mix, and their splitter reaches no mix in use.
- **Fix:** such a voice stays audible in the final mix's front pair (`audren_ipc.rs`, Turbo 8a265ab).

### Mixes missing from later updates

- **Cause:** since REV7 an update carries only the mixes and splitters that changed.
- **Fix:** mixes and splitters are kept by id between updates (Turbo a22b102).

### Opus decoded to silence

- **Symptom:** silence where a game plays Opus audio through the hardware decoder.
- **Cause:** Turbo answered the decoder without decoding before c406ae6.
- **Fix:** libopus decodes every packet (`codec.rs`; test `hwopus_ipc_decodes_real_opus_as_libopus`).

## Checking sound

- A healthy run logs `audio: output on ...` and RMS lines that are not silence; `metrics=1` adds underrun counts.
- PORT_PLAN 5A.2 records MK8D on WASAPI with 0 underruns over 68 s.
- AudioIn on a real microphone is not yet verified in a game (PORT_PLAN 5A.2, "AudioIn opens", Partial).

nxnative's notes on timing catch-up are covered by the catch-up frames above; its Apple audio-unit output and output
channel layouts have no counterpart (WASAPI folds to the device's own format).

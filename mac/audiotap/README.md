# mn-audiotap

macOS helper that records **system audio** (what the Mac plays, e.g. the other people in a call) and writes it to
stdout as raw **s16le, 16 kHz, mono** PCM, in blocks of about 100 ms. It is the "Them" track; the Rust core starts it
the same way it starts `parec` on Linux (`core/src/capture.rs`).

It uses Core Audio process taps, so it needs **macOS 14.2 or later**.

## Build

```sh
./build.sh          # universal arm64 + x86_64 release binary; the last line printed is its path
swift build         # quick debug build for the current Mac
```

Needs Xcode or the Command Line Tools (Swift 5.9+).

## Run

```sh
.build/debug/mn-audiotap > them.raw            # stop with Ctrl-C
ffplay -f s16le -ar 16000 -ac 1 them.raw       # play it back
```

No arguments. Stop it with SIGTERM or SIGINT, or by closing its stdout. It then stops the device and removes the
tap and the aggregate device, and exits 0. Errors go to stderr as one sentence, exit code 1.

The tap is private and unmuted: the user still hears the call, and no extra device shows up in Audio MIDI Setup.

## Permission

The first run triggers the macOS **System Audio Recording** prompt. macOS attributes it to the app that started the
tool (the parent), so that app's `Info.plist` needs `NSAudioCaptureUsageDescription`. If you run the tool from a
terminal, the terminal app is asked instead.

If the prompt was dismissed, allow the app under **System Settings › Privacy & Security › Screen & System Audio
Recording**, then start the tool again. Without permission macOS may still create the tap but deliver silence.

## Notes

- The aggregate device uses the default output device as its clock. If the user switches output device during a
  recording, the tap keeps following the old one; restart the tool to follow the new one.
- Downsampling is linear interpolation without a low-pass filter. That is fine for speech recognition.

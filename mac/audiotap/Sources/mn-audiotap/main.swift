// mn-audiotap: system audio ("Them" track) as raw s16le, 16 kHz, mono PCM on stdout, until killed.
// Read mac/audiotap/README.md for the permission note and how the parent app runs it.

import CoreAudio
import Foundation

/// Prints one plain sentence to stderr and exits 1.
func fail(_ message: String) -> Never {
    fputs("mn-audiotap: \(message)\n", stderr)
    exit(1)
}

// A closed pipe must come back as EPIPE from write(), not kill the process.
signal(SIGPIPE, SIG_IGN)

guard ProcessInfo.processInfo.isOperatingSystemAtLeast(OperatingSystemVersion(majorVersion: 14, minorVersion: 2, patchVersion: 0))
else { fail("system audio recording needs macOS 14.2 or later") }

let tap = SystemAudioTap()
let writer = StdoutWriter()
var finished = false

/// The only exit path after startup. Runs on the main queue only, so `finished` needs no lock.
func shutdown() {
    guard !finished else { return }
    finished = true
    tap.stop()
    exit(0)
}

do {
    try tap.prepare()
    let resampler = Resampler(inputRate: tap.sampleRate)
    // About every 100 ms: the IOProc converts the block and hands the bytes to the writer queue.
    try tap.begin { input in
        if let bytes = resampler.convert(input) { writer.send(bytes) }
    }
} catch let error as TapError {
    tap.stop()
    fail(error.message)
} catch {
    tap.stop()
    fail("could not start system audio capture: \(error.localizedDescription)")
}

// A failed write (reader gone) ends the run the same way a signal does.
writer.onFailure = { DispatchQueue.main.async { shutdown() } }

// Signals are ignored by default and handled on the main queue instead, so cleanup is not signal-unsafe.
// SIGPIPE stays ignored in effect; a source for it just gives a second way to reach the same shutdown.
let signalSources: [DispatchSourceSignal] = [SIGTERM, SIGINT, SIGPIPE].map { sig in
    signal(sig, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
    source.setEventHandler { shutdown() }
    source.resume()
    return source
}
_ = signalSources

dispatchMain()

import CoreAudio
import Foundation

/// A Core Audio failure with a plain-sentence message (printed after "mn-audiotap: ").
struct TapError: Error {
    let message: String
}

/// Turns a non-zero OSStatus into a TapError that names the failing call.
func check(_ status: OSStatus, _ call: String, hint: String? = nil) throws {
    guard status != noErr else { return }
    let extra = hint.map { "; \($0)" } ?? ""
    throw TapError(message: "\(call) failed with OSStatus \(status)\(extra)")
}

private func address(_ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain)
}

/// Owns the process tap, the private aggregate device that carries it, and the IOProc.
/// Every step records what it created, so `stop()` can undo exactly that, also after a half-finished start.
final class SystemAudioTap {
    /// Sample rate of the tap stream in Hz. Valid after `prepare()`.
    private(set) var sampleRate: Double = 0

    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private var started = false
    /// Real-time callbacks run here instead of on a Core Audio default thread.
    private let ioQueue = DispatchQueue(label: "mn-audiotap.io", qos: .userInteractive)

    /// Steps 1-4a: create the tap and the aggregate device, and read the tap's format.
    func prepare() throws {
        // Global stereo tap of every process's output. Private (only this process sees it) and unmuted,
        // so the user still hears the call.
        let description = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        description.name = "mn-audiotap"
        description.isPrivate = true
        description.muteBehavior = .unmuted
        try check(
            AudioHardwareCreateProcessTap(description, &tapID),
            "AudioHardwareCreateProcessTap",
            hint: "allow this app under System Settings > Privacy & Security > Screen & System Audio Recording")

        try readTapFormat()

        // The default output device is the main sub-device, so its clock drives the aggregate.
        let outputUID = try defaultOutputDeviceUID()
        // Explicit types: a mixed String/Bool literal nested inside `Any` does not type-check on its own.
        let tapEntry: [String: Any] = [
            kAudioSubTapUIDKey: description.uuid.uuidString,
            kAudioSubTapDriftCompensationKey: true,
        ]
        let aggregate: [String: Any] = [
            kAudioAggregateDeviceNameKey: "mn-audiotap",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: outputUID,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: outputUID]],
            kAudioAggregateDeviceTapListKey: [tapEntry],
        ]
        try check(
            AudioHardwareCreateAggregateDevice(aggregate as CFDictionary, &aggregateID),
            "AudioHardwareCreateAggregateDevice")
    }

    /// Steps 4b: install the IOProc and start the device. `handler` runs on `ioQueue` with the input buffers.
    func begin(handler: @escaping (UnsafePointer<AudioBufferList>) -> Void) throws {
        try check(
            AudioDeviceCreateIOProcIDWithBlock(&procID, aggregateID, ioQueue) { _, inInputData, _, _, _ in
                handler(inInputData)
            },
            "AudioDeviceCreateIOProcIDWithBlock")
        guard let procID else { throw TapError(message: "AudioDeviceCreateIOProcIDWithBlock returned no IOProc") }
        try check(AudioDeviceStart(aggregateID, procID), "AudioDeviceStart")
        started = true
    }

    /// Stops the device and destroys everything created so far, in reverse order. Safe to call twice.
    func stop() {
        if started, let procID {
            warn(AudioDeviceStop(aggregateID, procID), "AudioDeviceStop")
        }
        started = false
        if let procID, aggregateID != AudioObjectID(kAudioObjectUnknown) {
            warn(AudioDeviceDestroyIOProcID(aggregateID, procID), "AudioDeviceDestroyIOProcID")
        }
        procID = nil
        if aggregateID != AudioObjectID(kAudioObjectUnknown) {
            warn(AudioHardwareDestroyAggregateDevice(aggregateID), "AudioHardwareDestroyAggregateDevice")
            aggregateID = AudioObjectID(kAudioObjectUnknown)
        }
        if tapID != AudioObjectID(kAudioObjectUnknown) {
            warn(AudioHardwareDestroyProcessTap(tapID), "AudioHardwareDestroyProcessTap")
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
    }

    // MARK: - Property helpers

    /// Reads kAudioTapPropertyFormat and checks it is the float32 PCM this tool knows how to convert.
    private func readTapFormat() throws {
        var format = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var addr = address(kAudioTapPropertyFormat)
        try check(
            AudioObjectGetPropertyData(tapID, &addr, 0, nil, &size, &format),
            "AudioObjectGetPropertyData(kAudioTapPropertyFormat)")
        let isFloat32 =
            format.mFormatID == kAudioFormatLinearPCM
            && format.mFormatFlags & kAudioFormatFlagIsFloat != 0
            && format.mBitsPerChannel == 32
        guard isFloat32, format.mSampleRate > 0 else {
            throw TapError(message: "the system audio tap delivers an unsupported format (not 32-bit float PCM)")
        }
        sampleRate = format.mSampleRate
    }

    private func defaultOutputDeviceUID() throws -> String {
        var device = AudioObjectID(kAudioObjectUnknown)
        var size = UInt32(MemoryLayout<AudioObjectID>.size)
        var addr = address(kAudioHardwarePropertyDefaultOutputDevice)
        try check(
            AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &addr, 0, nil, &size, &device),
            "AudioObjectGetPropertyData(kAudioHardwarePropertyDefaultOutputDevice)")
        guard device != AudioObjectID(kAudioObjectUnknown) else {
            throw TapError(message: "there is no default audio output device")
        }

        // The UID comes back as a retained CFString that the caller must release.
        var uid: Unmanaged<CFString>?
        size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        addr = address(kAudioDevicePropertyDeviceUID)
        try check(
            AudioObjectGetPropertyData(device, &addr, 0, nil, &size, &uid),
            "AudioObjectGetPropertyData(kAudioDevicePropertyDeviceUID)")
        guard let uid else { throw TapError(message: "the default output device has no UID") }
        return uid.takeRetainedValue() as String
    }

    /// Cleanup failures are reported but never stop the rest of the cleanup.
    private func warn(_ status: OSStatus, _ call: String) {
        if status != noErr { fputs("mn-audiotap: \(call) failed with OSStatus \(status)\n", stderr) }
    }
}

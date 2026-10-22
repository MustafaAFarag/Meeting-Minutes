import CoreAudio
import Foundation

/// Float input buffers (any channel layout) -> 16 kHz mono s16le bytes.
/// Downmix is a plain average of all channels; resampling is linear interpolation. The position and the last
/// sample are kept between callbacks, so block edges do not click. Only the IOProc queue may call `convert`.
final class Resampler {
    private let step: Double  // input samples per output sample
    private var pos = 0.0  // next output position, in input samples; -1 is the last sample of the previous block
    private var prev: Float = 0
    private var mono: [Float] = []
    private var out: [Int16] = []

    init(inputRate: Double, outputRate: Double = 16_000) {
        step = inputRate / outputRate
        mono.reserveCapacity(16_384)  // avoids reallocation inside the real-time callback
        out.reserveCapacity(8_192)
    }

    /// Returns the s16le bytes for this block, or nil if the block has no usable samples.
    func convert(_ list: UnsafePointer<AudioBufferList>) -> Data? {
        downmix(list)
        let n = mono.count
        guard n > 0 else { return nil }

        out.removeAll(keepingCapacity: true)
        // Output sample at position p blends input samples floor(p) and floor(p)+1 (index -1 is `prev`).
        while true {
            let i = Int(pos.rounded(.down))
            if i + 1 > n - 1 { break }
            let a = i < 0 ? prev : mono[i]
            let b = mono[i + 1]
            let v = a + (b - a) * Float(pos - Double(i))
            // Int16(NaN) would trap, so non-finite samples become silence.
            out.append(v.isFinite ? Int16(max(-1, min(1, v)) * 32_767) : 0)
            pos += step
        }
        pos -= Double(n)
        prev = mono[n - 1]
        return out.withUnsafeBytes { Data($0) }  // little-endian on both arm64 and x86_64
    }

    /// Averages every channel of every buffer into `mono`. Handles interleaved (one buffer with N channels)
    /// and planar (N buffers with one channel each) layouts alike.
    private func downmix(_ list: UnsafePointer<AudioBufferList>) {
        mono.removeAll(keepingCapacity: true)
        let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: list))
        var frames = Int.max
        var channels = 0
        for buffer in buffers {
            let c = Int(buffer.mNumberChannels)
            guard c > 0, buffer.mData != nil else { return }
            frames = min(frames, Int(buffer.mDataByteSize) / (MemoryLayout<Float>.size * c))
            channels += c
        }
        guard channels > 0, frames > 0, frames != Int.max else { return }

        for f in 0..<frames {
            var sum: Float = 0
            for buffer in buffers {
                guard let data = buffer.mData else { continue }
                let samples = data.assumingMemoryBound(to: Float.self)
                let c = Int(buffer.mNumberChannels)
                for k in 0..<c { sum += samples[f * c + k] }
            }
            mono.append(sum / Float(channels))
        }
    }
}

/// Writes byte blocks to stdout from its own serial queue, so a slow reader never stalls the audio thread.
/// Uses write(2) directly: FileHandle.write raises an Objective-C exception on EPIPE instead of returning.
final class StdoutWriter {
    /// Called once, on the writer queue, when stdout can no longer be written (reader closed it).
    var onFailure: (() -> Void)?

    private let queue = DispatchQueue(label: "mn-audiotap.stdout", qos: .userInitiated)
    private var failed = false  // only touched on `queue`

    func send(_ data: Data) {
        queue.async { [self] in
            guard !failed else { return }
            if !writeAll(data) {
                failed = true
                onFailure?()
            }
        }
    }

    private func writeAll(_ data: Data) -> Bool {
        data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return true }
            var offset = 0
            while offset < raw.count {
                let n = write(STDOUT_FILENO, base + offset, raw.count - offset)
                if n > 0 {
                    offset += n
                } else if n < 0 && errno == EINTR {
                    continue
                } else {
                    return false  // EPIPE, EBADF, or a closed descriptor
                }
            }
            return true
        }
    }
}

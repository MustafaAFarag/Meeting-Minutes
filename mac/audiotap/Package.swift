// swift-tools-version:5.9
import PackageDescription

// A single command-line tool. Core Audio and Foundation ship with macOS, so there are no dependencies.
let package = Package(
    name: "mn-audiotap",
    platforms: [.macOS("14.2")],  // Core Audio process taps need 14.2
    targets: [
        .executableTarget(name: "mn-audiotap", path: "Sources/mn-audiotap")
    ]
)

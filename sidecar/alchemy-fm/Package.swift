// swift-tools-version:6.0
// Zero-dependency sidecar: Apple Foundation Models → NDJSON over stdio.
// Built by scripts/build-fm-sidecar.sh; bundled as a Tauri external binary.
// See docs/RFC-inference-providers.md §4 (first-party convergence).
import PackageDescription

let package = Package(
    name: "alchemy-fm",
    platforms: [.macOS(.v15)],
    targets: [
        .executableTarget(name: "alchemy-fm", path: "Sources/alchemy-fm")
    ]
)

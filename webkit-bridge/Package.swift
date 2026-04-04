// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "LadWebKitBridge",
    platforms: [.macOS(.v12)],
    targets: [
        .executableTarget(
            name: "lad-webkit-bridge",
            path: "Sources"
        ),
    ]
)

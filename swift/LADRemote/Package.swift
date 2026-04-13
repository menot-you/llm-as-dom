// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "LADRemote",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "LADRemote", targets: ["LADRemote"]),
    ],
    targets: [
        .target(
            name: "LADRemote",
            path: "Sources/LADRemote"
        ),
    ]
)

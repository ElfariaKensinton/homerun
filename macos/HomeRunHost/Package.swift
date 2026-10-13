// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "HomeRunHost",
    platforms: [.macOS(.v13)],
    products: [
        .executable(name: "HomeRunHost", targets: ["HomeRunHost"])
    ],
    targets: [
        .executableTarget(name: "HomeRunHost")
    ],
    swiftLanguageVersions: [.v5]
)

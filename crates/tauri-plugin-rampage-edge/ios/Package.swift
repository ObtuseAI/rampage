// swift-tools-version:5.9
import PackageDescription

let package = Package(
  name: "tauri-plugin-rampage-edge",
  platforms: [.iOS(.v15)],
  products: [.library(name: "tauri-plugin-rampage-edge", type: .static, targets: ["tauri-plugin-rampage-edge"])],
  dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")],
  targets: [.target(name: "tauri-plugin-rampage-edge", dependencies: [.byName(name: "Tauri")], path: "Sources")]
)


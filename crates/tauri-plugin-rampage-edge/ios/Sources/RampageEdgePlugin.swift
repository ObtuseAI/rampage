import Foundation
import SwiftRs
import Tauri
import UIKit
import WebKit

class DonationArgs: Decodable {
  let enabled: Bool
}

final class RampageEdgePlugin: Plugin {
  private var donationRequested = false
  private var observesLifecycle = false

  @objc public func status(_ invoke: Invoke) {
    DispatchQueue.main.async {
      self.observeLifecycle()
      invoke.resolve(self.readStatus())
    }
  }

  @objc public func setDonation(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(DonationArgs.self)
    DispatchQueue.main.async {
      self.observeLifecycle()
      self.donationRequested = args.enabled && UIApplication.shared.applicationState == .active
      UIApplication.shared.isIdleTimerDisabled = self.donationRequested
      invoke.resolve(self.readStatus())
    }
  }

  private func observeLifecycle() {
    guard !observesLifecycle else { return }
    observesLifecycle = true
    NotificationCenter.default.addObserver(
      forName: UIApplication.willResignActiveNotification,
      object: nil,
      queue: .main
    ) { [weak self] _ in
      self?.donationRequested = false
      UIApplication.shared.isIdleTimerDisabled = false
    }
  }

  private func readStatus() -> JsonObject {
    UIDevice.current.isBatteryMonitoringEnabled = true
    let rawBattery = UIDevice.current.batteryLevel
    let battery = rawBattery >= 0 ? Int((rawBattery * 100).rounded()) : 0
    let externalPower = UIDevice.current.batteryState == .charging || UIDevice.current.batteryState == .full
    let thermal: Int
    switch ProcessInfo.processInfo.thermalState {
    case .nominal: thermal = 100
    case .fair: thermal = 65
    case .serious: thermal = 25
    case .critical: thermal = 0
    @unknown default: thermal = 0
    }
    let foreground = UIApplication.shared.applicationState == .active && donationRequested
    return [
      "platform": "ios",
      "deviceKind": UIDevice.current.userInterfaceIdiom == .pad ? "tablet" : "phone",
      "foreground": foreground,
      "donationRequested": donationRequested,
      "batteryPercent": battery,
      "onExternalPower": externalPower,
      "lowPowerMode": ProcessInfo.processInfo.isLowPowerModeEnabled,
      "thermalHeadroomPercent": thermal,
      "screenKeptAwake": foreground && UIApplication.shared.isIdleTimerDisabled,
    ]
  }
}

@_cdecl("init_plugin_rampage_edge")
func initPlugin() -> Plugin {
  RampageEdgePlugin()
}

package ai.obtuse.rampage.edge

import android.app.Activity
import android.content.Context
import android.content.res.Configuration
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.view.WindowManager
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class DonationArgs {
    var enabled: Boolean = false
}

@TauriPlugin
class RampageEdgePlugin(private val activity: Activity) : Plugin(activity) {
    private var resumed = false
    private var donationRequested = false

    override fun load(webView: WebView) {
        super.load(webView)
        resumed = true
    }

    override fun onResume() {
        super.onResume()
        resumed = true
    }

    override fun onPause() {
        super.onPause()
        resumed = false
        donationRequested = false
        activity.runOnUiThread {
            activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    @Command
    fun status(invoke: Invoke) {
        invoke.resolve(readStatus())
    }

    @Command
    fun setDonation(invoke: Invoke) {
        val args = invoke.parseArgs(DonationArgs::class.java)
        donationRequested = args.enabled && resumed
        activity.runOnUiThread {
            if (donationRequested) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            }
            invoke.resolve(readStatus())
        }
    }

    private fun readStatus(): JSObject {
        val battery = activity.getSystemService(Context.BATTERY_SERVICE) as BatteryManager
        val power = activity.getSystemService(Context.POWER_SERVICE) as PowerManager
        val capacity = battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        val thermal = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            when (power.currentThermalStatus) {
                PowerManager.THERMAL_STATUS_NONE -> 100
                PowerManager.THERMAL_STATUS_LIGHT -> 80
                PowerManager.THERMAL_STATUS_MODERATE -> 55
                PowerManager.THERMAL_STATUS_SEVERE -> 30
                PowerManager.THERMAL_STATUS_CRITICAL -> 10
                else -> 0
            }
        } else {
            0
        }
        val deviceKind = if (
            activity.resources.configuration.smallestScreenWidthDp >= 600 ||
            (activity.resources.configuration.screenLayout and Configuration.SCREENLAYOUT_SIZE_MASK) >= Configuration.SCREENLAYOUT_SIZE_LARGE
        ) "tablet" else "phone"

        return JSObject().apply {
            put("platform", "android")
            put("deviceKind", deviceKind)
            put("foreground", resumed && donationRequested)
            put("donationRequested", donationRequested)
            put("batteryPercent", if (capacity in 0..100) capacity else 0)
            put("onExternalPower", battery.isCharging)
            put("lowPowerMode", power.isPowerSaveMode)
            put("thermalHeadroomPercent", thermal)
            put("screenKeptAwake", donationRequested)
        }
    }
}


use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeDeviceStatus {
    pub platform: String,
    pub device_kind: String,
    pub foreground: bool,
    pub donation_requested: bool,
    pub battery_percent: u8,
    pub on_external_power: bool,
    pub low_power_mode: bool,
    pub thermal_headroom_percent: u8,
    pub screen_kept_awake: bool,
}

impl EdgeDeviceStatus {
    pub fn eligible(&self, minimum_battery: u8, minimum_thermal_headroom: u8) -> bool {
        self.foreground
            && self.donation_requested
            && !self.low_power_mode
            && (self.on_external_power || self.battery_percent >= minimum_battery)
            && self.thermal_headroom_percent >= minimum_thermal_headroom
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DonationPayload {
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_fails_closed_on_every_mobile_pressure_signal() {
        let healthy = EdgeDeviceStatus {
            platform: "android".into(),
            device_kind: "phone".into(),
            foreground: true,
            donation_requested: true,
            battery_percent: 80,
            on_external_power: false,
            low_power_mode: false,
            thermal_headroom_percent: 80,
            screen_kept_awake: true,
        };
        assert!(healthy.eligible(40, 35));
        for mut denied in [
            EdgeDeviceStatus {
                foreground: false,
                ..healthy.clone()
            },
            EdgeDeviceStatus {
                donation_requested: false,
                ..healthy.clone()
            },
            EdgeDeviceStatus {
                battery_percent: 39,
                ..healthy.clone()
            },
            EdgeDeviceStatus {
                low_power_mode: true,
                ..healthy.clone()
            },
            EdgeDeviceStatus {
                thermal_headroom_percent: 34,
                ..healthy.clone()
            },
        ] {
            denied.on_external_power = false;
            assert!(!denied.eligible(40, 35));
        }
    }
}

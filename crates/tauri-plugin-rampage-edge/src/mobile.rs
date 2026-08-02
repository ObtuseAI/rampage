use serde::de::DeserializeOwned;
use tauri::{
    AppHandle, Runtime,
    plugin::{PluginApi, PluginHandle},
};

use crate::{DonationPayload, EdgeDeviceStatus, Result};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "ai.obtuse.rampage.edge";

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_rampage_edge);

pub struct RampageEdge<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> Result<RampageEdge<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "RampageEdgePlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_rampage_edge)?;
    Ok(RampageEdge(handle))
}

impl<R: Runtime> RampageEdge<R> {
    pub fn status(&self) -> Result<EdgeDeviceStatus> {
        self.0.run_mobile_plugin("status", ()).map_err(Into::into)
    }

    pub fn set_donation(&self, enabled: bool) -> Result<EdgeDeviceStatus> {
        self.0
            .run_mobile_plugin("setDonation", DonationPayload { enabled })
            .map_err(Into::into)
    }
}

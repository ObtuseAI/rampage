use std::marker::PhantomData;
use tauri::{AppHandle, Runtime, plugin::PluginApi};

use crate::{EdgeDeviceStatus, Error, Result};

pub struct RampageEdge<R: Runtime>(PhantomData<R>);

pub fn init<R: Runtime, C: serde::de::DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<RampageEdge<R>> {
    Ok(RampageEdge(PhantomData))
}

impl<R: Runtime> RampageEdge<R> {
    pub fn status(&self) -> Result<EdgeDeviceStatus> {
        Err(Error::UnsupportedPlatform)
    }

    pub fn set_donation(&self, _enabled: bool) -> Result<EdgeDeviceStatus> {
        Err(Error::UnsupportedPlatform)
    }
}

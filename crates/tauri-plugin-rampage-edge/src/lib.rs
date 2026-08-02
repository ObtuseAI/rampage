use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

#[cfg(desktop)]
mod desktop;
mod error;
#[cfg(mobile)]
mod mobile;
mod models;

#[cfg(desktop)]
pub use desktop::RampageEdge;
pub use error::{Error, Result};
#[cfg(mobile)]
pub use mobile::RampageEdge;
#[cfg(mobile)]
pub(crate) use models::DonationPayload;
pub use models::EdgeDeviceStatus;

pub trait RampageEdgeExt<R: Runtime> {
    fn rampage_edge(&self) -> &RampageEdge<R>;
}

impl<R: Runtime, T: Manager<R>> RampageEdgeExt<R> for T {
    fn rampage_edge(&self) -> &RampageEdge<R> {
        self.state::<RampageEdge<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("rampage-edge")
        .setup(|app, api| {
            #[cfg(mobile)]
            let edge = mobile::init(app, api)?;
            #[cfg(desktop)]
            let edge = desktop::init(app, api)?;
            app.manage(edge);
            Ok(())
        })
        .build()
}

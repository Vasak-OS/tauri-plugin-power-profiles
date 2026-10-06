//! Perfil de energía de power-profiles-daemon para aplicaciones de Tauri.
//!
//! Expone dos comandos (`get_power_state` y `set_power_profile`) y el evento
//! [`POWER_STATE_EVENT`], que se emite cada vez que el perfil cambia —desde
//! esta aplicación, desde otra o desde el demonio mismo—. No sondea: lo que
//! devuelve sale de una copia que mantienen las señales de D-Bus.

use std::sync::Arc;

use tauri::{
    plugin::{Builder, TauriPlugin},
    AppHandle, Emitter, Manager, Runtime,
};

mod commands;
mod desktop;
mod error;
mod models;

pub use desktop::PowerManager;
pub use error::{Error, Result};
pub use models::{PowerState, POWER_STATE_EVENT};

/// Acceso al gestor desde Rust, para una aplicación que quiera leer o cambiar
/// el perfil sin pasar por el frontend.
pub trait PowerManagerExt<R: Runtime> {
    fn power_manager(&self) -> &PowerManager;
}

impl<R: Runtime, T: Manager<R>> PowerManagerExt<R> for T {
    fn power_manager(&self) -> &PowerManager {
        self.state::<PowerManager>().inner()
    }
}

fn emitter<R: Runtime>(app: AppHandle<R>) -> desktop::Notify {
    Arc::new(move |state: &PowerState| {
        if let Err(e) = app.emit(POWER_STATE_EVENT, state) {
            log::warn!("power-manager: no se pudo emitir {POWER_STATE_EVENT}: {e}");
        }
    })
}

/// Inicializa el plugin.
///
/// La conexión con el bus se abre en segundo plano después del `setup`: la
/// ventana no espera al demonio, y para cuando el frontend pregunta, la copia
/// del estado ya suele estar lista.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::<R>::new("power-manager")
        .invoke_handler(tauri::generate_handler![
            commands::get_power_state,
            commands::set_power_profile,
        ])
        .setup(|app, _api| {
            app.manage(PowerManager::new(emitter(app.clone())));
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<PowerManager>().warm_up().await;
            });
            Ok(())
        })
        .build()
}

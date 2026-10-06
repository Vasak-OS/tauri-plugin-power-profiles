use tauri::State;

use crate::desktop::PowerProfiles;
use crate::models::PowerState;
use crate::Result;

/// El estado del perfil de energía. No va al bus: devuelve la copia que el
/// plugin mantiene con las señales del demonio. Sin demonio, `available` es
/// falso y no es un error.
#[tauri::command]
pub async fn get_power_state(state: State<'_, PowerProfiles>) -> Result<PowerState> {
    Ok(state.state().await)
}

/// Cambia el perfil activo y devuelve el estado nuevo.
#[tauri::command]
pub async fn set_power_profile(
    state: State<'_, PowerProfiles>,
    profile: String,
) -> Result<PowerState> {
    state.set_profile(&profile).await
}

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** Los perfiles que conoce power-profiles-daemon. Un equipo puede ofrecer menos. */
export type PowerProfile = 'power-saver' | 'balanced' | 'performance' | (string & {});

/**
 * El estado del perfil de energía. Sale de una copia que el plugin mantiene con
 * las señales del demonio: pedirlo no va al bus.
 */
export interface PowerState {
	/** Falso sin power-profiles-daemon: el selector va «no disponible», no roto. */
	available: boolean;
	/** Los perfiles de este equipo, en el orden del demonio. */
	profiles: PowerProfile[];
	activeProfile: PowerProfile | null;
	/** Por qué el rendimiento está limitado (`lap-detected`…), o `null`. */
	performanceDegraded: string | null;
}

/** El evento que se emite cuando cambia el perfil, venga de donde venga. */
export const POWER_STATE_EVENT = 'power-profile-changed';

/** El estado actual. Sin demonio no falla: devuelve `available: false`. */
export async function getPowerState(): Promise<PowerState> {
	return await invoke<PowerState>('plugin:power-manager|get_power_state');
}

/**
 * Cambia el perfil activo y devuelve el estado nuevo. Falla si el perfil no es
 * uno de los que ofrece el equipo o si no hay demonio.
 */
export async function setPowerProfile(profile: PowerProfile): Promise<PowerState> {
	return await invoke<PowerState>('plugin:power-manager|set_power_profile', { profile });
}

/**
 * Avisa cada vez que el perfil cambia: desde esta aplicación, desde otra o
 * desde el demonio (por ejemplo, al limitarse por temperatura). Devuelve la
 * función que deja de escuchar.
 */
export async function onPowerStateChanged(
	handler: (state: PowerState) => void,
): Promise<UnlistenFn> {
	return await listen<PowerState>(POWER_STATE_EVENT, (event) => handler(event.payload));
}

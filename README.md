# tauri-plugin-power-manager

Plugin de Tauri para leer y cambiar el **perfil de energía** del equipo
(`power-saver`, `balanced`, `performance`) a través de
[power-profiles-daemon](https://gitlab.freedesktop.org/upower/power-profiles-daemon)
por D-Bus, con un evento cuando el perfil cambia.

Es parte de VasakOS: lo usan `vasak-settings` y el centro de control de
`vasak-desktop`. Sólo Linux.

## Cómo funciona, y por qué es barato

- **Una sola lectura.** Al arrancar —en segundo plano, sin demorar la
  ventana— el plugin hace un `GetAll` de la interfaz del demonio y guarda el
  resultado. `get_power_state` devuelve esa copia: abrir el selector no va al
  bus.
- **Señales, no sondeo.** La copia se mantiene con `PropertiesChanged` (el
  perfil cambió desde otra aplicación, o el demonio limitó el rendimiento por
  temperatura) y `NameOwnerChanged` (el demonio se instaló, arrancó o se
  cayó). Cada cambio real se emite como el evento `power-profile-changed`.
- **Sin subprocesos.** Todo es D-Bus por `zbus`.
- **Los dos nombres del demonio.** Desde la 0.20 se publica como
  `org.freedesktop.UPower.PowerProfiles` y mantiene `net.hadess.PowerProfiles`
  por compatibilidad. El plugin prueba el nuevo primero y cae al viejo.
- **Sin demonio no es un error.** `get_power_state` devuelve
  `available: false` y la interfaz lo muestra «no disponible».

## Instalación

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri-plugin-power-manager = "2"
```

```sh
bun add @vasakgroup/plugin-power-manager
```

```rust
tauri::Builder::default()
    .plugin(tauri_plugin_power_manager::init())
```

Y en la capacidad de la ventana:

```json
"permissions": ["power-manager:default"]
```

`power-manager:default` permite los dos comandos. Escuchar el evento usa el
permiso `core:event:default`, que ya viene en `core:default`.

## Uso

```ts
import {
  getPowerState,
  setPowerProfile,
  onPowerStateChanged,
} from '@vasakgroup/plugin-power-manager';

const state = await getPowerState();
// { available: true, profiles: ['power-saver', 'balanced', 'performance'],
//   activeProfile: 'balanced', performanceDegraded: null }

await setPowerProfile('power-saver'); // devuelve el estado nuevo

const unlisten = await onPowerStateChanged((next) => {
  console.log(next.activeProfile);
});
```

| Comando | Devuelve | Notas |
|---|---|---|
| `get_power_state` | `PowerState` | De la copia; no va al bus. |
| `set_power_profile(profile)` | `PowerState` | Falla si el perfil no está en `profiles` o si no hay demonio. |

`performanceDegraded` trae la razón que da el demonio cuando limita el perfil
de rendimiento (`lap-detected`, `high-operating-temperature`), o `null`.

Desde Rust, el mismo gestor está en `app.power_manager()` (trait
`PowerManagerExt`).

## Pruebas

```sh
cargo test                   # las que no necesitan el bus
cargo test -- --ignored      # contra el demonio de esta máquina (cambia el perfil un instante y lo devuelve)
bun run test                 # el binding de JavaScript
```

## Licencia

GPL-3.0-or-later.

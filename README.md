# tauri-plugin-power-manager

Plugin de Tauri para leer y cambiar el **perfil de energía** del equipo
(`power-saver`, `balanced`, `performance`) a través de
[power-profiles-daemon](https://gitlab.freedesktop.org/upower/power-profiles-daemon)
por D-Bus, con un evento cuando el perfil cambia. Sin sondeo y sin
subprocesos.

Es parte de VasakOS: lo usan `vasak-settings` y el centro de control de
`vasak-desktop`.

> El código llega en el primer pull request. Este commit sólo crea el
> repositorio.

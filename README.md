# FrameRush

Herramienta en Rust para subir los FPS en juegos. Limita los procesos de fondo por niveles (**Normal**, **Agresivo** y **Extremo**), da prioridad al proceso del juego, gestiona el plan de energía y la frecuencia de la pantalla, y muestra la temperatura de la GPU y las RPM de los ventiladores. Detecta los juegos de Steam y tiene ajustes propios para Counter-Strike 2.

> **Estado: pre-release (0.1.0), solo Windows.** El instalador no está firmado, así que Windows SmartScreen mostrará un aviso. Linux y macOS tienen una base que compila y pasa los tests, pero todavía no aplican cambios.

## Qué hace

- **Niveles acumulativos**: Extremo incluye Agresivo, y Agresivo incluye Normal. El nivel solo propone qué marcar; lo que se aplica lo decides tú en la lista.
- **Procesos**: baja la prioridad, congela o (solo en Extremo) cierra procesos de fondo según reglas en `rules/*.toml`.
- **Juego**: prioridad del proceso, afinidad opcional y preferencia de GPU.
- **Sistema**: plan de energía propio («FrameRush Max»), frecuencia máxima de la pantalla y ajustes del registro con fuente documentada.
- **Turbo** («Jugar ahora»): lanza el juego desde Steam, congela lo marcado y vuelve a escanear mientras juegas.
- **Watchdog**: al cerrar el juego deshace los cambios de la sesión, también con la ventana de FrameRush cerrada.
- **Perfiles por juego**: guarda lo elegido para cada juego y puede aplicarlo solo al abrirlo desde Steam.
- **Sensores**: temperatura de la GPU (NVIDIA) y ventiladores. Lo que no se puede leer aparece como «No disponible», nunca como 0.
- Interfaz en español e inglés, con línea de comandos (`framerush`) para todo.

## Seguridad

- **Todo se puede restaurar.** Antes de cada cambio se guarda el estado anterior en un journal. **Restaurar** lo devuelve todo a como estaba, también después de un fallo o un reinicio. Cerrar procesos (solo en Extremo) es lo único que no se deshace.
- **Nunca toca** los procesos críticos del sistema, el audio, el compositor ni los anticheats. Esa lista no se puede cambiar desde las reglas ni desde la interfaz.
- **No desactiva Microsoft Defender.** Solo ofrece, si tú lo marcas, exclusiones para la carpeta del juego.
- La interfaz corre sin privilegios. Lo que necesita administrador pasa por un proceso auxiliar (`framerush-helper`) que solo acepta una lista cerrada de órdenes y se eleva con UAC cuando hace falta.
- Al desinstalar se ejecuta Restaurar.

## Compilar

Necesita la toolchain de Rust fijada en `rust-toolchain.toml` y las [dependencias de Tauri 2](https://v2.tauri.app/start/prerequisites/).

```powershell
cargo build --workspace
cargo test --workspace --all-features
cargo run -p framerush-gui                              # interfaz
cargo run -p framerush-cli -- plan --level agresivo     # simulación: no toca el sistema
```

Instalador de Windows (NSIS, sin firmar; necesita `tauri-cli`):

```powershell
pwsh -NoProfile -File scripts\package-windows.ps1
```

## Ramas

- `main`: solo releases, cada una con su tag `vX.Y.Z`.
- `develop`: desarrollo del día a día.

## Licencia

Doble licencia, a tu elección: [MIT](LICENSE-MIT) o [Apache-2.0](LICENSE-APACHE).

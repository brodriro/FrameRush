/*
 * Capa de datos de la UI: comandos y eventos de Tauri (contrato de design/README.md,
 * "Shared backend contract"). Todo sale del equipo donde corre la app (R8); no hay datos de
 * muestra. Fuera de Tauri no hay backend y load() falla.
 */
(function () {
  'use strict';

  const T = window.__TAURI__;
  const inTauri = !!T;

  window.FrameRushApi = {
    inTauri,

    /* Snapshot del equipo: DeviceProfile, capacidades, plan, protegidos y sesión pendiente. */
    async load() {
      if (!inTauri) throw new Error('Abre FrameRush como app para ver los datos de este equipo.');
      return T.core.invoke('snapshot');
    },

    /*
     * Evento `sensors` a 1 Hz: lista de { id, label, source, kind, value, unit, target }.
     * value null = "No disponible"; nunca se convierte en 0. Devuelve la función para dejar de escuchar.
     */
    onSensors(cb) {
      let stop = () => {};
      T.event.listen('sensors', e => cb(e.payload)).then(un => { stop = un; });
      return () => stop();
    },

    /* FPS: aún no hay medición (PresentMon / MangoHud llegan en M4), así que siempre null. */
    onFps(cb) {
      cb(null);
      return () => {};
    },

    /*
     * apply({ level, rows, settings, powerPlan, game, autoRevert }), undo() y revert(): informe
     * { lines: [{ what, outcome: done|skipped|failed, detail }], done, failed, notWired, needsAdmin, notes,
     *   sessionOpen, watching }. watching: el watchdog vigila el juego (solo en apply).
     */
    apply(selection) { return T.core.invoke('apply', { selection }); },

    /*
     * Evento `game-exited` del watchdog: el juego se cerró y se revirtió lo de sesión.
     * { outcomes: informe como el de revert() | null, error: texto | null }.
     */
    onGameExited(cb) {
      let stop = () => {};
      T.event.listen('game-exited', e => cb(e.payload)).then(un => { stop = un; });
      return () => stop();
    },
    /*
     * Evento `game-launched`: arrancó un juego de Steam (no cuenta el que ya corría al abrir la app).
     * { appid, pids }.
     */
    /*
     * Turbo (It5.2): launchGame pide a Steam que lance el juego (steam://rungameid) y devuelve
     * true si ya estaba en marcha (entonces no lo lanza). El evento
     * `turbo-rescan` llega en cada re-escaneo con { outcomes | null, error | null }.
     */
    launchGame(appid) { return T.core.invoke('launch_game', { appid }); },
    onTurboRescan(cb) {
      let stop = () => {};
      T.event.listen('turbo-rescan', e => cb(e.payload)).then(un => { stop = un; });
      return () => stop();
    },
    onGameLaunched(cb) {
      let stop = () => {};
      T.event.listen('game-launched', e => cb(e.payload)).then(un => { stop = un; });
      return () => stop();
    },
    undo() { return T.core.invoke('undo'); },

    /*
     * Perfiles por juego (It3.2): { [appid]: { name, data } }. data es el estado de la UI
     * (profileState en app.js); el backend lo guarda tal cual en profiles.json.
     */
    profiles() { return T.core.invoke('profiles'); },
    saveProfile(appid, name, data) { return T.core.invoke('save_profile', { appid, profile: { name, data } }); },
    revert() { return T.core.invoke('revert'); },

    /*
     * Reanudar una fila suelta (releaseRow) sin tocar el resto de la sesión, volver a limitarla
     * con la acción del nivel (limitRow; nunca cierra) y los ids de las filas limitadas ahora.
     */
    releaseRow(id) { return T.core.invoke('release_row', { id }); },
    limitRow(id, level) { return T.core.invoke('limit_row', { id, level }); },
    limitedRows() { return inTauri ? T.core.invoke('limited_rows') : Promise.resolve([]); },

    /* Abre el helper elevado (UAC) y vuelve a abrir los sensores a través de él. */
    elevate() {
      if (!inTauri) return Promise.reject(new Error('Solo dentro de la app.'));
      return T.core.invoke('elevate');
    },

    /* Ventana de Tauri (barra propia: la ventana no tiene bordes). */
    win(action, arg) {
      if (!inTauri) return Promise.resolve();
      const w = T.window.getCurrentWindow();
      if (action === 'minimize') return w.minimize();
      if (action === 'close') return w.close();
      if (action === 'always-on-top') return w.setAlwaysOnTop(arg);
      return Promise.resolve();
    },
  };
})();

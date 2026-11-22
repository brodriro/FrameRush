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
     * apply({ level, rows, settings, powerPlan }), undo() y revert(): informe
     * { lines: [{ what, outcome: done|skipped|failed, detail }], done, failed, notWired, needsAdmin, sessionOpen }.
     */
    apply(selection) { return T.core.invoke('apply', { selection }); },
    undo() { return T.core.invoke('undo'); },
    revert() { return T.core.invoke('revert'); },

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

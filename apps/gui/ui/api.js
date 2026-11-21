/*
 * Capa de datos de la UI. Hoy sirve los DATOS DE MUESTRA de sample-data.js y simula
 * los eventos del backend; cuando exista el crate de Tauri (apps/gui/src-tauri),
 * estas funciones pasarán a llamar a los comandos y eventos del contrato de
 * design/README.md ("Shared backend contract") y app.js no tendrá que cambiar.
 */
(function () {
  'use strict';

  const SAMPLE = window.FRAMERUSH_SAMPLE;
  const inTauri = !!window.__TAURI__;

  /* Copia profunda: la UI nunca muta el snapshot original. */
  const clone = x => JSON.parse(JSON.stringify(x));

  /* Ruido de la simulación de sensores (solo maqueta). */
  const jitter = (base, amp) => base + Math.round(Math.random() * 2 * amp - amp);

  window.FrameRushApi = {
    inTauri,
    /* Claves de los perfiles de muestra (solo para la barra de desarrollo). */
    sampleKeys: Object.keys(SAMPLE),

    /*
     * Snapshot inicial: DeviceProfile, capacidades, juegos, plan, reglas, sesión pendiente.
     * Futuro: capabilities(), detect_game(), list_steam_games(), plan(...), pending_session().
     */
    async load(sampleKey) {
      return clone(SAMPLE[sampleKey] || SAMPLE.win);
    },

    /*
     * Evento `sensors` a 1 Hz. Cada lectura es número o null ("No disponible");
     * un null nunca se convierte en 0. Futuro: listen('sensors', ...).
     */
    onSensors(snapshot, cb) {
      const s = snapshot.sensors;
      const read = () => cb({
        cpu: s.cpu.temp == null ? null : jitter(s.cpu.temp, 3),
        gpu: s.gpu.temp == null ? null : jitter(s.gpu.temp, 3),
        fans: s.fans.map(f => f.rpm),
      });
      read();
      const h = setInterval(read, 1000);
      return () => clearInterval(h);
    },

    /* Evento `fps` a 1 Hz; null si el SO no tiene medición. Futuro: listen('fps', ...). */
    onFps(snapshot, cb) {
      const read = () => cb(snapshot.caps.fps ? jitter(70, 5) : null);
      read();
      const h = setInterval(read, 1000);
      return () => clearInterval(h);
    },
  };
})();

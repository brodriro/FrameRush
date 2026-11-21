/*
 * DATOS DE MUESTRA. NO SON DATOS REALES DEL EQUIPO.
 *
 * Simulan lo que el backend enviará por IPC de Tauri (DeviceProfile de crates/device,
 * plan de crates/core, reglas, Steam, sensores y adaptadores de juego). La UI se
 * renderiza solo a partir de este objeto, así que sustituirlo por los datos reales
 * es cambiar api.js, no app.js (regla R8: nada estático).
 *
 * Son los mismos tres perfiles simulados que usa design/v3-overlay-compact.html:
 * un portátil Windows con Optimus, un portátil Linux con nouveau (GPU sin
 * temperatura) y un Mac con Apple Silicon.
 */
(function () {
  'use strict';

  /* Adaptador de juego (crates/games). Solo CS2 tiene adaptador por ahora. */
  const CS2_ADAPTER = {
    name: 'CS2',
    cfg: 'cs2_video.txt según el hardware: texturas bajas con 4 GB de VRAM, MSAA off, Reflex on',
    bench: 'fpsbench.cfg · en consola: exec fpsbench',
    launchOptions: '-console +exec framerush -fullscreen',
  };
  /* Portada simulada; en la app vendrá de la biblioteca de Steam. */
  const CS2_COVER = 'linear-gradient(135deg,#f2df3f 0 50%,#222 50%)';

  /* Juegos (crates/steam): el primero es el que está en ejecución. exe por SO. */
  const GAMES = [
    { name: 'Counter-Strike 2', appid: 730, adapter: CS2_ADAPTER, cover: CS2_COVER, running: true, pid: 48211,
      exe: { win: 'cs2.exe', lin: 'cs2 (nativo)', mac: 'cs2' } },
    { name: 'Counter-Strike 2 · FACEIT', appid: 730, adapter: CS2_ADAPTER, cover: CS2_COVER,
      antiCheat: { name: 'FACEIT AC', process: 'faceit', mayRequireVbs: true },
      exe: { win: 'cs2.exe + faceit.exe', lin: '—', mac: '—' } },
    { name: 'Cyberpunk 2077', appid: 1091500,
      exe: { win: 'Cyberpunk2077.exe', lin: 'Cyberpunk2077.exe · Proton 9.0', mac: '—' } },
    { name: 'Apex Legends', appid: 1172470, antiCheat: { name: 'Easy Anti-Cheat', process: 'EasyAntiCheat' },
      exe: { win: 'r5apex.exe', lin: 'r5apex.exe · Proton', mac: '—' } },
    { name: 'Elden Ring', appid: 1245620, antiCheat: { name: 'Easy Anti-Cheat', process: 'EasyAntiCheat_EOS' },
      exe: { win: 'eldenring.exe', lin: 'eldenring.exe · Proton', mac: '—' } },
  ];

  /* Recomendaciones para el juego (rules/ filtradas por appid, PLAN §6). dependsOn: índice de otra fila. */
  const STEAM_RECS = [
    { name: 'Desactivar overlay de Steam', why: 'Conflicto con frame generation en este juego', source: 'Juego', checked: true },
    { name: 'Limitar steamwebhelper ×5', why: 'Interfaz de la biblioteca (600 MB)', source: 'Steam', checked: true, dependsOn: 0 },
    { name: 'Pausar pre-cacheo de shaders', why: 'Compilación en segundo plano', source: 'Steam', checked: true },
    { name: 'MangoHud duplicado', why: 'Dos capas de overlay (gamescope + MangoHud)', source: 'Conflicto conocido', checked: false },
    { name: 'Overlay de Discord', why: 'Overlay duplicado con el de Steam', source: 'Conflicto conocido', checked: false },
  ];

  /* Recomendaciones manuales (manual-recommendations), filtradas por el equipo en app.js. */
  const MANUAL = [
    { gpuVendor: 'nvidia', text: 'Panel de NVIDIA → el juego: energía «Preferir rendimiento máximo», V-Sync off, baja latencia off (Reflex ya lo hace)' },
    { gpuVendor: 'amd', text: 'Adrenalin → el juego: Anti-Lag on, Radeon Chill off, V-Sync off' },
    { oem: 'Acer', text: 'NitroSense: ventilador al máximo' },
    { oem: 'Lenovo', text: 'Lenovo Vantage: modo rendimiento' },
    { battery: true, text: 'Juega enchufado; en portátil, usa la salida de vídeo de la GPU dedicada' },
    { hybridGpu: true, text: 'GPU híbrida: comprueba que el juego corre en la GPU dedicada' },
    { text: 'Actualiza el driver de la GPU' },
    { appid: 730, text: 'CS2: deja activo «Aumentar contraste de jugadores» aunque cueste algo de FPS' },
  ];

  /* Filas del checklist (plan de crates/core sobre el snapshot). system: corre como SYSTEM (requiere admin). */
  const PROCS_WIN = [
    { name: 'CompatTelRunner', exe: 'CompatTelRunner.exe', desc: 'Telemetría de compatibilidad · corre como SYSTEM', category: 'telemetry', ramMb: 38, cpu: 0.6, risk: 'low', action: 'suspend', minLevel: 'normal', system: true },
    { name: 'GoogleUpdate', exe: 'GoogleUpdate.exe', desc: 'Actualizador de Google · corre como SYSTEM', category: 'updater', ramMb: 12, cpu: 0.1, risk: 'low', action: 'suspend', minLevel: 'normal', system: true },
    { name: 'AdobeARM', exe: 'AdobeARM.exe', desc: 'Actualizador de Adobe', category: 'updater', ramMb: 9, cpu: 0.1, risk: 'low', action: 'suspend', minLevel: 'normal' },
    { name: 'Discord', exe: 'Discord.exe', desc: 'Menos prioridad; la voz sigue activa (no se congela en Turbo)', category: 'overlay', ramMb: 420, cpu: 2.5, risk: 'low', action: 'lower_priority', minLevel: 'normal' },
    { name: 'OneDrive', exe: 'OneDrive.exe', desc: 'Sincronización en la nube', category: 'sync', ramMb: 160, cpu: 1.1, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'Dropbox', exe: 'Dropbox.exe', desc: 'Sincronización Dropbox', category: 'sync', ramMb: 240, cpu: 0.9, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'Epic Games Launcher', exe: 'EpicGamesLauncher.exe', desc: 'Launcher ajeno al juego', category: 'launcher', ramMb: 310, cpu: 1.4, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'EA app', exe: 'EADesktop.exe', desc: 'Launcher ajeno al juego', category: 'launcher', ramMb: 280, cpu: 1, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'chrome ×14', exe: 'chrome.exe', desc: 'Opcional: 14 pestañas', category: 'browser', ramMb: 1720, cpu: 4.2, risk: 'medium', action: 'suspend', minLevel: 'agresivo', optional: true },
    { name: 'msedge ×6', exe: 'msedge.exe', desc: 'Opcional: WebView y pestañas', category: 'browser', ramMb: 640, cpu: 1.3, risk: 'medium', action: 'suspend', minLevel: 'agresivo', optional: true },
    { name: 'Spotify', exe: 'Spotify.exe', desc: 'Música en segundo plano', category: 'browser', ramMb: 290, cpu: 1.2, risk: 'low', action: 'suspend', minLevel: 'agresivo', optional: true },
    { name: 'iCUE', exe: 'iCUE.exe', desc: 'Control RGB (no crítico)', category: 'peripheral', ramMb: 210, cpu: 1.6, risk: 'medium', action: 'suspend', minLevel: 'agresivo' },
    { name: 'OBS (replay)', exe: 'obs64.exe', desc: 'Búfer de repetición', category: 'overlay', ramMb: 510, cpu: 4.1, risk: 'low', action: 'kill', minLevel: 'extremo' },
    { name: 'G HUB', exe: 'lghub.exe', desc: 'Riesgo alto: se suspende', category: 'peripheral', ramMb: 180, cpu: 0.8, risk: 'high', action: 'kill', minLevel: 'extremo' },
    { name: 'Windows Search', exe: 'WSearch', desc: 'Indexador: el mayor consumidor de CPU/disco en segundo plano', category: 'service_a', ramMb: 120, cpu: 2.4, risk: 'low', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'SysMain', exe: 'SysMain', desc: 'Superfetch', category: 'service_a', ramMb: 60, cpu: 1.1, risk: 'low', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'Telemetría', exe: 'DiagTrack', desc: 'Experiencias del usuario conectado', category: 'service_a', ramMb: 30, cpu: 0.5, risk: 'low', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'Servicios Xbox', exe: 'XblAuthManager · XblGameSave · XboxNetApiSvc', desc: 'Solo si no usas Xbox / Game Pass', category: 'service_a', ramMb: 40, cpu: 0.2, risk: 'medium', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'Diagnósticos', exe: 'DPS · WdiServiceHost · WdiSystemHost', desc: 'Directivas de diagnóstico', category: 'service_a', ramMb: 26, cpu: 0.3, risk: 'low', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'Informes de errores', exe: 'WerSvc · PcaSvc', desc: 'Errores y asistente de compatibilidad', category: 'service_a', ramMb: 18, cpu: 0.2, risk: 'low', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'Windows Update', exe: 'wuauserv · BITS · UsoSvc · WaaSMedicSvc', desc: 'Evita descargas a mitad de partida', category: 'service_b', ramMb: 90, cpu: 1.8, risk: 'medium', action: 'stop_service', minLevel: 'extremo' },
    { name: 'Cola de impresión', exe: 'Spooler', desc: 'Impresión', category: 'service_b', ramMb: 12, cpu: 0.1, risk: 'low', action: 'stop_service', minLevel: 'extremo' },
    { name: 'UPnP / SSDP', exe: 'SSDPSRV · upnphost', desc: 'Descubrimiento de red', category: 'service_b', ramMb: 10, cpu: 0.1, risk: 'low', action: 'stop_service', minLevel: 'extremo' },
    { name: 'Ubicación y ICS', exe: 'lfsvc · SharedAccess', desc: 'Geolocalización y conexión compartida', category: 'service_b', ramMb: 14, cpu: 0.1, risk: 'low', action: 'stop_service', minLevel: 'extremo' },
  ];

  /* Nombres de Linux/macOS sin fuente verificada todavía (PLAN V20); son solo de la maqueta. */
  const PROCS_LIN = [
    { name: 'whoopsie', exe: 'whoopsie', desc: 'Informes de errores de Ubuntu', category: 'telemetry', ramMb: 14, cpu: 0.1, risk: 'low', action: 'suspend', minLevel: 'normal' },
    { name: 'ubuntu-report', exe: 'ubuntu-report', desc: 'Telemetría de instalación', category: 'telemetry', ramMb: 10, cpu: 0.1, risk: 'low', action: 'suspend', minLevel: 'normal' },
    { name: 'gnome-software', exe: 'gnome-software', desc: 'Comprobación de actualizaciones', category: 'updater', ramMb: 180, cpu: 1.2, risk: 'low', action: 'suspend', minLevel: 'normal' },
    { name: 'update-notifier', exe: 'update-notifier', desc: 'Avisos de actualización', category: 'updater', ramMb: 32, cpu: 0.2, risk: 'low', action: 'suspend', minLevel: 'normal' },
    { name: 'discord', exe: 'discord', desc: 'Menos prioridad; la voz sigue activa', category: 'overlay', ramMb: 420, cpu: 2.5, risk: 'low', action: 'lower_priority', minLevel: 'normal' },
    { name: 'nextcloud', exe: 'nextcloud', desc: 'Sincronización Nextcloud', category: 'sync', ramMb: 210, cpu: 1.2, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'dropbox', exe: 'dropbox', desc: 'Sincronización Dropbox', category: 'sync', ramMb: 240, cpu: 0.9, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'heroic', exe: 'heroic', desc: 'Heroic Games Launcher', category: 'launcher', ramMb: 330, cpu: 1.6, risk: 'low', action: 'suspend', minLevel: 'agresivo' },
    { name: 'lutris', exe: 'lutris', desc: 'Lutris en segundo plano', category: 'launcher', ramMb: 180, cpu: 0.6, risk: 'medium', action: 'suspend', minLevel: 'agresivo' },
    { name: 'firefox ×11', exe: 'firefox', desc: 'Opcional: 11 pestañas', category: 'browser', ramMb: 1480, cpu: 3.9, risk: 'medium', action: 'suspend', minLevel: 'agresivo', optional: true },
    { name: 'chromium ×8', exe: 'chromium', desc: 'Opcional: pestañas y extensiones', category: 'browser', ramMb: 920, cpu: 2.2, risk: 'medium', action: 'lower_priority', minLevel: 'agresivo', optional: true },
    { name: 'openrgb', exe: 'openrgb', desc: 'Control RGB (no crítico)', category: 'peripheral', ramMb: 90, cpu: 0.7, risk: 'medium', action: 'suspend', minLevel: 'agresivo' },
    { name: 'obs (replay)', exe: 'obs', desc: 'Búfer de repetición de OBS', category: 'overlay', ramMb: 510, cpu: 4.1, risk: 'low', action: 'kill', minLevel: 'extremo' },
    { name: 'piper', exe: 'piper', desc: 'Riesgo alto: se suspende', category: 'peripheral', ramMb: 60, cpu: 0.3, risk: 'high', action: 'kill', minLevel: 'extremo' },
    { name: 'tracker-miner-fs-3', exe: 'tracker-miner-fs-3.service', desc: 'Indexador de archivos', category: 'service_a', ramMb: 140, cpu: 2.3, risk: 'medium', action: 'stop_service', minLevel: 'agresivo' },
    { name: 'packagekit', exe: 'packagekit.service', desc: 'Servicio de paquetes', category: 'service_b', ramMb: 96, cpu: 1.8, risk: 'low', action: 'stop_service', minLevel: 'extremo' },
    { name: 'cups', exe: 'cups.service', desc: 'Servicio de impresión', category: 'service_b', ramMb: 18, cpu: 0.1, risk: 'low', action: 'stop_service', minLevel: 'extremo' },
  ];
  const PROCS_MAC = PROCS_LIN.filter(p => !/whoopsie|ubuntu|gnome|update-notifier|tracker|packagekit/.test(p.name));

  /* Antes/después medido (PresentMon/MangoHud): 13 puntos en un lienzo de 396×60. */
  const PERF = {
    before: { fps: 72, low1: 41, series: [30, 26, 40, 18, 34, 28, 42, 16, 32, 27, 39, 20, 31] },
    after: { fps: 79, low1: 58, series: [44, 43, 46, 40, 45, 44, 46, 41, 45, 44, 46, 42, 44] },
  };

  /* Un perfil completo por SO: lo que devolverá el backend al arrancar. */
  function profile(os, d) {
    return Object.assign({
      os,
      games: GAMES.map(g => Object.assign({}, g, { exe: g.exe[os] })),
      steamRecs: STEAM_RECS,
      manual: MANUAL,
      perf: PERF,
      profiles: [{ id: 'global', name: 'Global' }, { id: 'cp2077', name: 'Cyberpunk 2077' }],
      pendingSession: { changes: 6 },   /* journal sin cerrar → restore-after-crash */
      elevated: false,
    }, d);
  }

  window.FRAMERUSH_SAMPLE = {
    win: profile('win', {
      caps: { affinity: true, timer: true, gpu: 'write', fps: 'presentmon', windows: true },
      device: {
        cpu: { model: 'i5-9300H', physical: 4, threads: 8, smt: 2, eCores: 0, tjmax: 100 },
        gpus: [{ name: 'GTX 1650', vramGb: 4, kind: 'dgpu', slowdown: 93 }, { name: 'UHD 630', kind: 'igpu' }],
        hybridTech: 'Optimus', memoryGb: 16, gpuVendor: 'nvidia', oem: 'Acer',
        bottleneck: 'CPU en mapas cargados, GPU en 1080p',
        display: { hz: { current: 60, max: 75 }, modes: [{ w: 1920, h: 1080 }, { w: 1680, h: 1050 }, { w: 1280, h: 960 }, { w: 1280, h: 720 }] },
        battery: { present: true, discharging: true },
        power: { backend: 'Plan de energía de Windows', plans: ['Equilibrado', 'Alto rendimiento', 'Economizador'], current: 0, best: 1 },
        vbsActive: true, hagsSupported: true, windows11: true, defenderActive: true,
        voiceApps: ['discord'], virtualization: ['WSL2', 'Docker Desktop'],
      },
      sensors: { cpu: { source: 'LHM (PawnIO)', temp: 75 }, gpu: { source: 'NVML', temp: 68 },
        fans: [{ name: 'Ventilador CPU', rpm: 3400 }, { name: 'Ventilador GPU', rpm: 3150, target: true }] },
      processes: PROCS_WIN,
      protected: [{ name: 'csrss / wininit / lsass', why: 'núcleo del sistema' }, { name: 'dwm', why: 'compositor' }, { name: 'audiodg', why: 'audio' }, { name: 'explorer', why: 'nunca se finaliza' }],
    }),
    lin: profile('lin', {
      caps: { affinity: true, timer: false, gpu: 'info', fps: 'mangohud', windows: false },
      device: {
        cpu: { model: 'Ryzen 7 6800H', physical: 8, threads: 16, smt: 2, eCores: 0, tjmax: null },
        gpus: [{ name: 'RTX 3070', vramGb: 8, kind: 'dgpu', slowdown: null }, { name: 'Radeon 680M', kind: 'igpu' }],
        hybridTech: null, memoryGb: 32, gpuVendor: 'nvidia', oem: 'Lenovo',
        bottleneck: 'GPU',
        display: { hz: { current: 165, max: 165 }, modes: [{ w: 2560, h: 1600 }, { w: 1920, h: 1200 }, { w: 1920, h: 1080 }] },
        battery: { present: true, discharging: false },
        power: { backend: 'power-profiles-daemon (D-Bus)', plans: ['power-saver', 'balanced', 'performance'], current: 1, best: 2 },
        vbsActive: false, hagsSupported: false, windows11: false, defenderActive: false,
        voiceApps: ['discord', 'mumble'], virtualization: [],
      },
      sensors: { cpu: { source: 'k10temp', temp: 75 }, gpu: { source: 'nouveau', temp: null },
        fans: [{ name: 'Ventilador CPU', rpm: 1510 }, { name: 'Ventilador GPU', rpm: 2450 }, { name: 'Caja 1', rpm: 870 }, { name: 'Caja 2', rpm: 0 }, { name: 'Caja 3', rpm: null }] },
      processes: PROCS_LIN,
      protected: [{ name: 'systemd', why: 'PID 1' }, { name: 'pipewire', why: 'audio' }, { name: 'kwin_wayland', why: 'compositor' }, { name: 'Xwayland', why: 'servidor X' }],
    }),
    mac: profile('mac', {
      caps: { affinity: false, timer: false, gpu: false, fps: false, windows: false },
      device: {
        cpu: { model: 'Apple M2 Pro', physical: 12, threads: 12, smt: 1, eCores: 4, tjmax: null },
        gpus: [{ name: 'M2 Pro · 19 núcleos', kind: 'integrated', slowdown: null }],
        hybridTech: null, memoryGb: 16, unifiedMemory: true, gpuVendor: 'apple', oem: 'Apple',
        bottleneck: 'GPU',
        display: { hz: { current: 120, max: 120 }, modes: [{ w: 3024, h: 1964 }, { w: 1512, h: 982 }] },
        battery: { present: true, discharging: false },
        power: { backend: 'pmset lowpowermode (requiere helper)', plans: ['Bajo consumo: sí', 'Bajo consumo: no'], current: 1, best: 1 },
        vbsActive: false, hagsSupported: false, windows11: false, defenderActive: false,
        voiceApps: [], virtualization: [],
      },
      sensors: { cpu: { source: 'IOHID', temp: 75 }, gpu: { source: 'IOKit', temp: 61 }, fans: [{ name: 'Ventilador', rpm: 1200 }] },
      processes: PROCS_MAC,
      protected: [{ name: 'kernel_task', why: 'núcleo' }, { name: 'WindowServer', why: 'compositor' }, { name: 'coreaudiod', why: 'audio' }],
    }),
  };
})();

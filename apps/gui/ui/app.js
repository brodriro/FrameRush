/*
 * FrameRush GUI (diseño final: design/v3-overlay-compact.html).
 * Ids de data-setting, valores y defaults: design/SETTINGS.md. Reglas de comportamiento: SETTINGS.md §4 y §6.
 * Todo lo que depende del equipo llega en el snapshot de FrameRushApi.load() (R8); aquí solo hay
 * textos de la interfaz y reglas del plan.
 */
(function () {
  'use strict';

  const api = window.FrameRushApi;

  /* ---------- Utilidades de DOM ---------- */
  const $ = s => document.querySelector(s);
  const $$ = s => [...document.querySelectorAll(s)];
  const DS = id => $$(`[data-setting="${id}"]`);
  const first = id => DS(id)[0];
  const rowOf = id => first(id)?.closest('.row');
  const setText = (sel, txt) => $$(sel).forEach(e => { e.textContent = txt; });
  const esc = v => String(v).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  /* Visible = ni oculto por capacidades del SO ([hidden]) ni porque el equipo no lo tiene ([data-dev-hidden], P15). */
  const shown = el => !!el && !el.closest('[hidden],[data-dev-hidden]');
  const active = id => { const x = first(id); return !!x && x.checked && !x.disabled && shown(x); };

  /* ---------- Textos de la interfaz y reglas del plan (no dependen del equipo) ---------- */
  const CAT = { telemetry: 'Telemetría', updater: 'Actualizadores', sync: 'Sync / nube', launcher: 'Launchers', browser: 'Navegadores y apps', overlay: 'Overlays', peripheral: 'Periféricos', service_a: 'Servicios · grupo A (Agresivo)', service_b: 'Servicios · grupo B (Extremo)' };
  const ACT = { lower_priority: 'Bajar prioridad', suspend: 'Suspender', kill: 'Finalizar', stop_service: 'Detener servicio' };
  const RISK = { low: ['Bajo', 1], medium: ['Medio', 2], high: ['Alto', 3] };
  const LV = { normal: 1, agresivo: 2, extremo: 3 };
  const LV_NAME = ['', 'Normal', 'Agresivo', 'Extremo'];
  const LV_NOTE = ['',
    'Normal (sin admin): baja la prioridad de lo no esencial, suspende actualizadores y telemetría y aplica los ajustes de usuario (Game DVR, notificaciones, efectos visuales). Energía sin cambios.',
    'Agresivo (admin, sin reinicio): + suspende sync, launchers y periféricos; servicios del grupo A; prioridad Alta persistente; plan propio; refresco al máximo; config del juego bloqueada.',
    '⚠ Extremo (requiere reinicio): + cierra procesos de riesgo bajo, servicios del grupo B, HAGS, VBS/HVCI off y C-states off. Afinidad y timer siguen siendo opt-in.'];
  const DEFAULT_LEVEL = 2; /* Agresivo (PLAN §3, valores por defecto) */
  /* Ajustes que requieren reinicio (SETTINGS §4.7). */
  const REBOOT = { hags: 'HAGS', 'vbs-hvci-off': 'VBS/HVCI off', 'cstates-off': 'C-states off', 'timer-resolution': 'timer global' };
  /* run-as-admin: W y L según SETTINGS.md; la etiqueta de macOS no está definida (pendiente). */
  const ELEVATE_LABEL = { win: '🔒 Ejecutar como administrador', lin: '🔒 Elevar (polkit)', mac: '🔒 Elevar' };
  const RT_API = { lin: ' (SCHED_RR)' };
  const FPS_SOURCE = { presentmon: 'PresentMon (ETW)', mangohud: 'Logs de MangoHud' };
  const GPU_NOTE = { write: 'Windows: UserGpuPreferences', info: 'Linux: DRI_PRIME / prime-run (solo informativo)' };
  const SENSOR_NOTE = { win: ' PawnIO se instala aparte: sin él solo hay GPU + temperatura ACPI.', mac: ' Apple Silicon: temperatura por API privada (experimental).' };
  /* Fallbacks "estimado" cuando no se puede leer TjMax ni el umbral de la GPU (PLAN It6). */
  const FALLBACK_ALERT = 85;
  const WARN_BELOW_ALERT = 15;
  /* Afinidad: aviso con 4 núcleos físicos o menos (PLAN It5.3). */
  const FEW_CORES = 4;
  /* Escala visual de la barra de RPM, tomada del mockup v3. Pendiente: no hay RPM máximas en el DeviceProfile. */
  const FAN_BAR_FULL_RPM = 4000;

  /* ---------- Estado ---------- */
  let D = null;           /* snapshot del backend (hoy, datos de muestra) */
  let stopStreams = [];
  const S = { level: DEFAULT_LEVEL, admin: false, game: 0, plan: 0, hist: [], rt: false, alerted: false, rb: [], turbo: false, fps: [], chk: [], stc: [] };

  const game = () => D.games[S.game];
  const lvlOf = p => LV[p.minLevel];
  /* SETTINGS §4.2: kill solo en Extremo y con riesgo bajo; si no, se suspende. */
  const effAct = p => (p.action === 'kill' && (S.level < 3 || p.risk !== 'low') ? 'suspend' : p.action);
  /* SETTINGS §4.3 y P12: servicios y procesos de SYSTEM necesitan el helper elevado. */
  const locked = p => (p.action === 'stop_service' || p.system) && !S.admin;
  const fmt = mb => (mb >= 1024 ? (mb / 1024).toFixed(1) + ' GB' : mb + ' MB');

  function toast(msg) {
    let t = $('#toast');
    if (!t) { t = document.createElement('div'); t.id = 'toast'; t.className = 'toast'; t.setAttribute('role', 'status'); document.body.appendChild(t); }
    t.textContent = msg; t.classList.add('show');
    clearTimeout(t._h); t._h = setTimeout(() => t.classList.remove('show'), 3200);
  }
  const setCheck = (id, v) => DS(id).forEach(x => { x.checked = v; });

  /* ---------- Plantillas ---------- */
  const actBadge = p => {
    const a = effAct(p), fell = a !== p.action;
    return `<span class="act" data-setting="process-action" title="${fell ? 'Finalizar solo en Extremo y con riesgo bajo: se suspende' : ACT[a]}">${ACT[a]}${fell ? ' *' : ''}</span>`;
  };
  const lockBadge = p => (locked(p) ? ' <span class="adm" data-setting="admin-lock">🔒 Requiere admin</span>' : '');
  const riskBadge = p => { const r = RISK[p.risk]; return `<span class="badge r${r[1]}">${r[0]}</span>`; };
  const rowHTML = (p, i) => `<label class="p prow ${S.chk[i] ? '' : 'off'} ${locked(p) ? 'lk' : ''}" title="${esc(p.desc)}"><input type="checkbox" data-i="${i}" ${S.chk[i] ? 'checked' : ''} ${locked(p) ? 'disabled' : ''}><span class="n"><b>${esc(p.name)}</b>${p.optional ? ' <span class="opt">opcional</span>' : ''}${lockBadge(p)}<i>${esc(p.desc)} · ${esc(p.exe)}</i></span><span class="r">${actBadge(p)} ${fmt(p.ramMb)} · ${p.cpu.toFixed(1)}%${riskBadge(p)}</span></label>`;
  const catHTML = c => `<div class="cat">${CAT[c]}<span class="sp"></span><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="1">todos</button><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="0">ninguno</button></div>`;
  const lockHTML = l => `<div class="p lk" data-setting="protected-processes"><span>🔒</span><span class="n"><b>${esc(l.name)}</b><i>${esc(l.why)} · nunca se modifica</i></span><span class="r"><span class="badge r0">Protegido</span></span></div>`;
  const stHTML = (s, i, dis) => `<label class="p ${dis ? 'dis' : ''}"><input type="checkbox" data-s="${i}" ${S.stc[i] ? 'checked' : ''} ${dis ? 'disabled' : ''}><span class="n"><b>${esc(s.name)}</b><span class="src">${esc(s.source)}</span><i>${esc(s.why)}${dis ? ' · requiere overlay desactivado' : ''}</i></span><span></span></label>`;

  /* ---------- Nivel (SETTINGS §4.1) ---------- */
  function setLevel(l) {
    S.level = l;
    D.processes.forEach((p, i) => { S.chk[i] = lvlOf(p) <= l && !locked(p) && !p.optional; });
    $$('[data-l]').forEach(b => b.classList.toggle('on', +b.dataset.l === l));
    $$('.lvNote').forEach(n => { n.textContent = LV_NOTE[l]; n.dataset.lv = l; });
    const pw = D.device.power;
    S.plan = [pw.current, pw.current, pw.best, pw.best][l];
    $$('.row[data-min]').forEach(r => {
      const ok = LV[r.dataset.min] <= l, x = r.querySelector('input');
      r.classList.toggle('na-lv', !ok);
      if (x) { x.disabled = !ok; x.checked = ok && !('optin' in r.dataset); }
      const s = r.querySelector('small');
      if (s && !ok && !s.dataset.o) s.dataset.o = s.textContent;
      if (s && s.dataset.o) s.textContent = ok ? s.dataset.o : 'Disponible desde ' + LV_NAME[LV[r.dataset.min]];
    });
    const g = game(), ac = g.antiCheat;
    if (ac) setCheck('vbs-hvci-off', false); /* P10: desmarcado con cualquier anticheat */
    DS('vbs-hvci-off').forEach(x => {
      if (ac && ac.mayRequireVbs) x.disabled = true; /* P10, V7: bloqueado con Vanguard/FACEIT */
      const n = x.closest('.row')?.querySelector('.vbsAc');
      if (n) { n.hidden = !(ac && ac.mayRequireVbs); n.textContent = n.hidden ? '' : `Bloqueado: ${ac.name} puede exigir VBS/HVCI`; }
    });
    if (priorityValue() === 'normal') setCheck('game-priority-persistent', false); /* P1 */
    renderPower(); render(); caps();
  }
  const priorityValue = () => $('[data-setting="game-priority"] button.on')?.dataset.v;

  /* ---------- Checklist de procesos y Steam ---------- */
  function render() {
    let h = '';
    for (const c in CAT) {
      const ix = D.processes.map((_, i) => i).filter(i => D.processes[i].category === c);
      if (!ix.length) continue;
      h += catHTML(c);
      ix.forEach(i => { h += rowHTML(D.processes[i], i); });
    }
    const ac = game().antiCheat;
    h += '<div class="cat">Protegidos (inmutables)<span class="sp"></span></div>';
    D.protected.concat(ac ? [{ name: ac.name + ' (anticheat)', why: ac.process }] : []).forEach(l => { h += lockHTML(l); });
    $('#rows').innerHTML = h;
    D.steamRecs.forEach((s, i) => { if (s.dependsOn != null && !S.stc[s.dependsOn]) S.stc[i] = false; });
    $('#steamList').innerHTML = D.steamRecs.map((s, i) => stHTML(s, i, s.dependsOn != null && !S.stc[s.dependsOn])).join('');
    summary();
  }

  function summary() {
    let n = 0, ram = 0, cpu = 0;
    D.processes.forEach((p, i) => { if (S.chk[i]) { n++; ram += p.ramMb; cpu += p.cpu; } });
    /* impact-estimate: solo RAM y CPU, nunca cifras de FPS (PLAN §6). */
    DS('impact-estimate').forEach(e => { e.innerHTML = `Liberación estimada: <b>${fmt(ram)}</b> RAM · <b>${cpu.toFixed(1)} %</b> CPU · ${n} elementos`; });
    setText('.pm', `${n}/${D.processes.length} marcados`);
    setText('.stm', `${S.stc.filter(Boolean).length}/${D.steamRecs.length} recomendaciones`);
  }

  /* ---------- Energía (P3) ---------- */
  function renderPower() {
    const o = D.device.power;
    DS('power-plan').forEach(g => {
      g.innerHTML = o.plans.map((x, i) => `<button data-i="${i}" class="${i === S.plan ? 'on' : ''}" title="${i === o.current ? 'Plan actual del sistema' : ''}">${esc(x)}${i === o.current ? ' •' : ''}</button>`).join('');
    });
    setText('.pwrCur', o.plans[o.current]);
    const custom = D.caps.windows && active('custom-power-plan');
    const what = custom ? `se activará «FrameRush Max» (base: ${o.plans[S.plan]})`
      : S.plan === o.current ? 'sin cambios en este nivel' : `se cambiará a «${o.plans[S.plan]}» al aplicar`;
    setText('.pwrNote', `${o.backend} · ${what} · se restaura al revertir`);
  }

  /* ---------- Capacidades del SO y juego seleccionado ---------- */
  function caps() {
    const k = D.caps, g = game(), ac = !!g.antiCheat;
    const capOf = { win: k.windows, affinity: k.affinity, timer: k.timer, gpu: k.gpu };
    $$('[data-cap]').forEach(e => { e.hidden = !capOf[e.dataset.cap]; });
    DS('gpu-preference').forEach(s => { s.disabled = k.gpu === 'info'; });
    setText('.gpuNote', GPU_NOTE[k.gpu] || '');
    /* SETTINGS §4.4: el anticheat desactiva afinidad y timer. */
    ['cpu-affinity', 'timer-resolution'].forEach(id => DS(id).forEach(x => { x.disabled = ac; if (ac) x.checked = false; }));
    DS('anti-cheat-warning').forEach(e => {
      e.hidden = !ac;
      e.textContent = ac ? `🛡 Anticheat detectado (${g.antiCheat.name}): afinidad y timer desactivados; el juego solo recibe prioridad y el anticheat está protegido.` : '';
    });
    setText('.fpsNote', k.fps ? FPS_SOURCE[k.fps] : 'macOS: no hay medición de FPS disponible');
    setText('.gameName', g.name); setText('.gameExe', g.exe); setText('.gameId', g.appid);
    setText('.gamePid', g.running && g.pid ? ` · PID ${g.pid}` : '');
    $$('.cover').forEach(e => { e.style.background = g.cover || ''; });
    $$('[data-adapter]').forEach(e => { e.hidden = !g.adapter; }); /* SETTINGS §4.11 */
    if (g.adapter) {
      setText('.adName', g.adapter.name); setText('.adCfg', g.adapter.cfg);
      setText('.adBench', g.adapter.bench); setText('.adOpts', g.adapter.launchOptions);
    }
    device();
  }

  /* ---------- DeviceProfile → opciones, defaults y visibilidad (R8, P15) ---------- */
  function hideRow(id, v) { const r = rowOf(id); if (r) r.toggleAttribute('data-dev-hidden', !!v); }

  function device() {
    const d = D.device, s = D.sensors, g = game(), cpu = d.cpu;
    DS('battery-warning').forEach(e => { e.hidden = !(d.battery.present && d.battery.discharging); });
    hideRow('hags', !d.hagsSupported);
    hideRow('vbs-hvci-off', !d.vbsActive);
    hideRow('defender-exclusions', !d.defenderActive);
    hideRow('timer-resolution', !d.windows11);
    hideRow('refresh-rate-max', d.display.hz.current >= d.display.hz.max);
    hideRow('gpu-preference', d.gpus.length < 2);
    setText('.hzNote', `Este monitor: ${d.display.hz.current} Hz → ${d.display.hz.max} Hz`);

    /* hardware-profile */
    const cores = cpu.eCores ? `${cpu.physical - cpu.eCores}P + ${cpu.eCores}E` : `${cpu.physical} núcleos físicos / ${cpu.threads} hilos`;
    const gpus = d.gpus.map((x, i) => {
      const n = esc(x.name) + (x.vramGb ? ` ${x.vramGb} GB` : '');
      return (i === 0 ? `<b>${n}</b>` : n) + (x.kind === 'integrated' ? ' (integrada)' : '');
    }).join(' + ') + (d.hybridTech ? ` (${esc(d.hybridTech)})` : '');
    DS('hardware-profile').forEach(e => {
      e.innerHTML = `<b>${esc(cpu.model)}</b> · ${cores} · ${gpus} · ${d.memoryGb} GB${d.unifiedMemory ? ' unificada' : ''}<br><span class="bn">Cuello de botella: ${esc(d.bottleneck)}</span>`;
    });

    /* gpu-preference: valores canónicos alto (dGPU) · ahorro (iGPU) · sistema; por defecto alto */
    DS('gpu-preference').forEach(sel => {
      const val = { dgpu: 'alto', igpu: 'ahorro' };
      sel.innerHTML = d.gpus.filter(x => val[x.kind]).map(x => `<option value="${val[x.kind]}">${esc(x.name)}${x.vramGb ? ` ${x.vramGb} GB` : ''} (${x.kind === 'dgpu' ? 'dGPU' : 'iGPU'})</option>`).join('') + '<option value="sistema">Sistema</option>';
    });

    /* resolution-override: nativa · modos reales del monitor (4:3 marcado) · custom */
    DS('resolution-override').forEach(sel => {
      const [nat, ...rest] = d.display.modes, wh = m => `${m.w}×${m.h}`;
      sel.innerHTML = `<option value="nativa">Nativa (${wh(nat)})</option>` +
        rest.map(m => `<option value="${m.w}x${m.h}">${wh(m)}${m.w * 3 === m.h * 4 ? ' (4:3 estirado)' : ''}</option>`).join('') +
        '<option value="custom">Personalizada…</option>';
    });

    const ka = first('keep-alive-list');
    if (ka && !ka.dataset.touched) ka.value = d.voiceApps.join(', ');

    /* Umbrales térmicos: min(TjMax − 10, slowdown GPU − 5); alerta − 15 para el aviso (PLAN It6) */
    const slowdown = d.gpus.find(x => x.slowdown != null)?.slowdown ?? null;
    const cands = [cpu.tjmax != null ? cpu.tjmax - 10 : null, slowdown != null ? slowdown - 5 : null].filter(x => x != null);
    const ta = cands.length ? Math.min(...cands) : null;
    const al = first('thermal-alert-threshold'), wa = first('thermal-warn-threshold');
    if (al && !al.dataset.touched) al.value = ta ?? FALLBACK_ALERT;
    if (wa && !wa.dataset.touched) wa.value = (ta ?? FALLBACK_ALERT) - WARN_BELOW_ALERT;
    setText('.thrNote', ta == null ? 'Umbrales estimados: no se pudo leer TjMax ni el umbral de la GPU.'
      : `Calculado: TjMax ${cpu.tjmax ?? '—'} °C − 10 · slowdown GPU ${slowdown ?? '—'} °C − 5 → ${ta} °C`);

    /* Sensores: fuentes y lista de ventiladores (las lecturas llegan por onSensors) */
    setText('.cpuSrc', s.cpu.source); setText('.gpuSrc', s.gpu.source);
    DS('sensor-fans').forEach(e => {
      e.innerHTML = '<div class="fan"><span>CPU</span><div class="b"><i data-meter="cpu"></i></div><span class="v" data-temp="cpu"></span></div>' +
        s.fans.map((f, i) => `<div class="fan"><span>${esc(f.name)}</span><div class="b" data-fanbar="${i}"></div><span class="v" data-fan="${i}"></span></div>`).join('');
    });
    DS('sensor-backend-notice').forEach(e => {
      e.textContent = `Detectado: CPU vía ${s.cpu.source}, GPU vía ${s.gpu.source}${s.gpu.temp == null ? ' (este driver no expone temperatura)' : ''}.${SENSOR_NOTE[D.os] || ''}`;
    });

    /* manual-recommendations filtradas por el equipo y el juego */
    const hybrid = d.gpus.length > 1;
    DS('manual-recommendations').forEach(e => {
      e.innerHTML = D.manual.filter(m => (!m.gpuVendor || m.gpuVendor === d.gpuVendor) && (!m.oem || m.oem === d.oem) && (!m.battery || d.battery.present) && (!m.hybridGpu || hybrid) && (!m.appid || m.appid === g.appid))
        .map(m => `<label><input type="checkbox"> <span>${esc(m.text)}</span></label>`).join('');
    });

    /* compat-warning: solo lo que está instalado */
    const ac = g.antiCheat;
    $$('.cwList').forEach(e => {
      e.innerHTML = (d.virtualization.length ? d.virtualization.map(v => `<li>${esc(v)} (instalado)</li>`).join('') : '<li>No se detectó WSL2, Docker ni emuladores instalados</li>') +
        (ac && ac.mayRequireVbs ? `<li>${esc(ac.name)} puede exigir VBS/HVCI: la opción queda bloqueada</li>` : ac ? `<li>Anticheat ${esc(ac.name)}: comprueba que no exija VBS</li>` : '');
    });

    setText('.affNote', cpu.physical <= FEW_CORES ? `⚠ Con ${cpu.physical} núcleos físicos la afinidad manual suele empeorar el rendimiento` : 'Opt-in en todos los niveles');
    renderAffinity();
    if (S.lastSensors) onSensors(S.lastSensors);
  }

  /* cpu-affinity-cores: todos menos el primer núcleo físico (y sus hilos SMT) y los E-cores */
  function renderAffinity() {
    const b = first('cpu-affinity-cores'); if (!b) return;
    const c = D.device.cpu, pCores = c.physical - c.eCores;
    b.innerHTML = [...Array(c.threads)].map((_, i) => {
      const core = Math.floor(i / c.smt), isE = core >= pCores, on = core > 0 && !isE;
      return `<input type="checkbox" id="core${i}" ${on ? 'checked' : ''}><label for="core${i}" title="Núcleo físico ${core}${c.smt > 1 ? ' · hilo ' + i : ''}">${isE ? 'E' : 'P'}${i}</label>`;
    }).join('');
    syncAffinityCores();
  }
  function syncAffinityCores() {
    const aff = first('cpu-affinity'), ac = !!game().antiCheat;
    $$('[data-setting="cpu-affinity-cores"] input').forEach(x => { x.disabled = ac || !(aff && aff.checked); });
  }

  /* ---------- Rendimiento (antes/después) ---------- */
  function renderPerf() {
    const has = !!D.caps.fps, pf = D.perf;
    const pts = s => s.map((v, i) => `${(i * 396 / (s.length - 1)).toFixed(1)},${v}`).join(' ');
    $('#perfBefore').setAttribute('points', has ? pts(pf.before.series) : '');
    $('#perfAfter').setAttribute('points', has ? pts(pf.after.series) : '');
    setText('.perfSum', has ? `antes ${pf.before.fps} FPS / 1% ${pf.before.low1} · después ${pf.after.fps} FPS / 1% ${pf.after.low1}` : 'sin medición');
  }

  /* ---------- Sesión ---------- */
  function rebootBanner() {
    DS('reboot-required').forEach(e => { e.hidden = !S.rb.length; const l = e.querySelector('.rbList'); if (l) l.textContent = S.rb.map(x => REBOOT[x]).join(', '); });
  }

  function statusText() {
    const pw = D.device.power, g = game();
    return [
      `Nivel         : ${LV_NAME[S.level]}${S.hist.length ? ' (aplicado)' : ' (sin aplicar)'}`,
      `Procesos      : ${S.chk.filter(Boolean).length} marcados`,
      `Plan energía  : ${pw.plans[S.plan]}${active('custom-power-plan') ? ' + FrameRush Max' : ''}`,
      `Prioridad     : ${S.rt ? 'Tiempo real' : priorityValue() === 'normal' ? 'Normal' : 'Alta'}${active('game-priority-persistent') ? ' (IFEO persistente)' : ''}`,
      `VBS / HVCI    : ${active('vbs-hvci-off') ? 'OFF (tras reinicio)' : 'ON'}`,
      `Refresco      : ${active('refresh-rate-max') ? 'máximo' : 'sin cambios'}`,
      `Config juego  : ${g.adapter ? (active('game-config-lock') ? 'ajustada + solo lectura' : 'ajustada') : 'sin adaptador'}`,
      `Turbo         : ${S.turbo ? 'activo' : 'inactivo'} · no congelar: ${first('keep-alive-list')?.value || '—'}`,
      `Reinicio      : ${S.rb.length ? S.rb.join(', ') : 'no'}`,
    ].join('\n');
  }

  function sess() {
    const on = S.hist.length > 0, ar = first('auto-revert-on-exit');
    DS('session-status').forEach(e => {
      e.classList.toggle('on', on);
      const t = e.querySelector('.sessT');
      if (t) t.textContent = on ? `Sesión activa · ${S.hist.reduce((a, b) => a + b, 0)} cambios · ${S.hist.length} aplicación(es)${ar && ar.checked ? ' · se revierte al cerrar el juego' : ''}` : 'Sin sesión · nada modificado';
    });
    DS('restore').forEach(b => { b.disabled = !on; });
    DS('undo-last').forEach(b => { b.disabled = !on; });
  }

  const levelToggles = () => $$('.tg[data-setting]').filter(x => x.checked && !x.disabled && shown(x) && x.closest('[data-min]'));

  /* IPC futuro: apply(plan). Simulado: cuenta los cambios y los "registra" en la sesión. */
  function apply(force) {
    const n = S.chk.filter(Boolean).length + S.stc.filter(Boolean).length + levelToggles().length;
    if (first('dry-run')?.checked) { toast(`Simulación: se aplicarían ${n} cambios. No se ha modificado nada.`); return; }
    if (!force && active('vbs-hvci-off')) { /* SETTINGS §4.8 */
      DS('compat-warning').forEach(e => { e.hidden = false; });
      toast('Confirma el aviso de compatibilidad para aplicar.');
      return;
    }
    S.rb = Object.keys(REBOOT).filter(active); rebootBanner();
    S.hist.push(n); sess();
    toast(`Aplicado: ${n} cambios registrados en el journal (backup original conservado).`);
  }

  /* IPC futuro: launch_game / Turbo (SETTINGS §4.9). */
  function launchGame() {
    const g = game();
    const keep = (first('keep-alive-list')?.value || '').split(',').map(s => s.trim().toLowerCase()).filter(Boolean);
    const frozen = D.processes.filter((p, i) => S.chk[i] && (p.action === 'suspend' || p.action === 'kill') && !keep.some(k => p.exe.toLowerCase().includes(k))).length; /* P6 */
    if (first('dry-run')?.checked) { toast(`Simulación: se lanzaría ${g.name} y se congelarían ${frozen} procesos. No se ha modificado nada.`); return; } /* P11 */
    S.turbo = true; S.hist.push(frozen); sess();
    toast(`steam://rungameid/${g.appid} · Turbo: ${frozen} procesos congelados${keep.length ? ' (excluidos: ' + keep.join(', ') + ')' : ''}, re-escaneo cada ${first('turbo-rescan').value} s. Se reanudan al cerrar ${g.name}.`);
  }

  /* ---------- Segmentados ---------- */
  function onSeg(id, b) {
    if (id === 'power-plan') { S.plan = +b.dataset.i; renderPower(); }
    if (id === 'perf-metric') setText('.chartLbl', b.textContent);
    if (id === 'game-priority') {
      const box = $('#rtBox');
      if (b.dataset.v === 'realtime' && !S.rt) {
        /* Doble confirmación: se queda en Alta hasta confirmar (It1.5) */
        [...b.parentElement.children].forEach(x => x.classList.toggle('on', x.dataset.v === 'alta'));
        box.hidden = false;
      } else {
        box.hidden = true;
        if (b.dataset.v !== 'realtime') S.rt = false;
      }
      if (b.dataset.v === 'normal') { setCheck('game-priority-persistent', false); toast('Prioridad Normal: la prioridad Alta persistente (IFEO) se desactiva.'); }
    }
  }

  /* ---------- Sensores y FPS (eventos) ---------- */
  function onSensors(r) {
    S.lastSensors = r;
    const w = +(first('thermal-warn-threshold')?.value || 0), a = +(first('thermal-alert-threshold')?.value || 0);
    ['cpu', 'gpu'].forEach(k => {
      const t = r[k], c = t == null ? null : t >= a ? 'bad' : t >= w ? 'warn' : 'ok';
      $$(`[data-temp="${k}"]`).forEach(e => {
        e.className = e.className.replace(/\s*(c-\w+|na)\b/g, '');
        if (t == null) { e.textContent = 'No disponible'; e.classList.add('na'); } else { e.textContent = t + ' °C'; e.classList.add('c-' + c); }
      });
      $$(`[data-meter="${k}"]`).forEach(m => { m.style.width = t == null ? '0' : Math.min(t, 100) + '%'; m.className = c ? 'bg-' + c : ''; });
      if (c === 'bad' && first('thermal-alerts')?.checked && !S.alerted) { S.alerted = true; toast(`${k.toUpperCase()} a ${t} °C: supera el umbral de alerta. Sugerencia: bajar a Normal.`); }
    });
    r.fans.forEach((rpm, i) => {
      $$(`[data-fan="${i}"]`).forEach(e => {
        const tgt = D.sensors.fans[i].target;
        e.className = 'v' + (rpm == null ? ' na' : rpm === 0 ? ' c-bad' : '');
        e.innerHTML = rpm == null ? 'No disponible' : rpm === 0 ? '0 rpm · parado'
          : rpm.toLocaleString('es') + ' rpm' + (tgt ? ' <span class="na" title="NVML da las RPM objetivo, no medidas">objetivo</span>' : '');
      });
      $$(`[data-fanbar="${i}"]`).forEach(b => { b.innerHTML = rpm ? `<i class="bg-ok" style="width:${Math.min(100, rpm / FAN_BAR_FULL_RPM * 100)}%"></i>` : ''; });
    });
    $$('.fanCount').forEach(e => {
      const ok = r.fans.filter(x => x != null).length;
      e.innerHTML = r.fans.length ? `${ok}<span class="na"> / ${r.fans.length}</span>` : '<span class="na">No disponible</span>';
    });
  }

  function onFps(v) {
    setText('.fpsNow', v == null ? '—' : String(v));
    const sp = $('#spk'); if (!sp) return;
    if (v == null) { S.fps = []; sp.setAttribute('points', ''); return; }
    S.fps.push(v); if (S.fps.length > 60) S.fps.shift();
    const W = +sp.dataset.w, H = +sp.dataset.h;
    sp.setAttribute('points', S.fps.map((x, i) => `${(i * W / 59).toFixed(1)},${(H - (x - 45) / 55 * H).toFixed(1)}`).join(' '));
  }

  /* ---------- Eventos de la UI ---------- */
  document.addEventListener('click', e => {
    const t = e.target;
    const dev = t.closest('.devbar [data-os]'); if (dev) { init(dev.dataset.os); return; }
    const lb = t.closest('[data-l]'); if (lb) { setLevel(+lb.dataset.l); return; }
    const bk = t.closest('[data-setting="process-category-bulk"]');
    if (bk) { e.preventDefault(); D.processes.forEach((p, i) => { if (p.category === bk.dataset.c && !locked(p)) S.chk[i] = bk.dataset.v === '1'; }); render(); return; }
    const sg = t.closest('.win .seg>button');
    if (sg && !sg.disabled) { const g = sg.parentElement; [...g.children].forEach(x => x.classList.toggle('on', x === sg)); onSeg(g.dataset.setting, sg); }
    if (t.closest('[data-cwok]')) {
      if (!$('#cwAck').checked) { toast('Marca «Entiendo lo que deja de funcionar».'); return; }
      DS('compat-warning').forEach(x => { x.hidden = true; }); $('#cwAck').checked = false; apply(true); return;
    }
    if (t.closest('[data-cw]')) { DS('compat-warning').forEach(x => { x.hidden = true; }); return; }
    if (t.closest('[data-rb]')) { DS('reboot-required').forEach(x => { x.hidden = true; }); return; }
    const cr = t.closest('[data-crash]');
    if (cr) {
      DS('restore-after-crash').forEach(x => { x.hidden = true; });
      toast(cr.dataset.crash === 'restore' ? `Sesión anterior restaurada: ${D.pendingSession.changes} cambios revertidos.` : 'Aviso descartado. El journal se conserva hasta el próximo Restaurar.');
      return;
    }
    const wb = t.closest('[data-win]');
    if (wb) { if (!api.inTauri) toast('Minimizar y cerrar solo funcionan dentro de la app.'); else api.win(wb.dataset.win); return; } /* La bandeja llega con el overlay (M6) */

    const el = t.closest('button[data-setting]'); if (!el) return;
    const id = el.dataset.setting;
    if (id === 'apply') apply();
    if (id === 'undo-last') { S.hist.pop(); sess(); toast('Última aplicación revertida.'); }
    if (id === 'restore') {
      const rb = S.rb.length; S.hist = []; S.turbo = false; sess();
      toast('Todo restaurado al estado original: procesos, servicios, registro, plan de energía, pantalla y config del juego.' + (rb ? ' Reinicia para revertir los ajustes de arranque.' : ''));
    }
    if (id === 'launch-game') launchGame();
    if (id === 'launch-options-copy') toast(`Copiado al portapapeles: ${game().adapter?.launchOptions || ''}`);
    if (id === 'status-view') { const v = $('#stv'); v.hidden = !v.hidden; if (!v.hidden) v.textContent = statusText(); }
    if (id === 'realtime-confirm') {
      if (!$('#rtAck').checked) { toast('Marca «Entiendo el riesgo» para confirmar.'); return; }
      S.rt = true; $$('[data-setting="game-priority"] button').forEach(x => x.classList.toggle('on', x.dataset.v === 'realtime'));
      $('#rtBox').hidden = true; $('#rtAck').checked = false; toast('Prioridad Tiempo real confirmada (doble confirmación).');
    }
    if (id === 'run-as-admin') { S.admin = true; elevateButton(); setLevel(S.level); } /* IPC futuro: elevate() */
    if (id === 'profile-save') toast(`Perfil guardado para ${game().name} (nivel, checklist, juego y energía).`);
    if (id === 'export-diagnostics') toast('Diagnóstico exportado (logs locales, sin telemetría).');
    if (id === 'always-on-top') { const on = el.getAttribute('aria-pressed') !== 'true'; el.setAttribute('aria-pressed', on); el.classList.toggle('on', on); api.win('always-on-top', on); }
  });

  document.addEventListener('input', e => {
    if (['keep-alive-list', 'thermal-alert-threshold', 'thermal-warn-threshold'].includes(e.target.dataset.setting)) e.target.dataset.touched = 1;
  });

  document.addEventListener('change', e => {
    const t = e.target, id = t.dataset.setting;
    if (t.dataset.i != null) { S.chk[+t.dataset.i] = t.checked; t.closest('.prow')?.classList.toggle('off', !t.checked); summary(); return; }
    if (t.dataset.s != null) { S.stc[+t.dataset.s] = t.checked; render(); return; }
    if (id === 'game-target') { S.game = +t.value; setLevel(S.level); toast(`Juego: ${game().name}`); }
    if (id === 'cpu-affinity') syncAffinityCores();
    if (id === 'custom-power-plan') renderPower();
    if (id === 'game-priority-persistent' && t.checked && priorityValue() === 'normal') { t.checked = false; toast('IFEO solo persiste «Alta»: elige Alta primero.'); }
    if (id === 'vbs-hvci-off' && t.checked) toast('VBS/HVCI off: al aplicar se pedirá confirmación de compatibilidad.');
    if (id === 'fps-cap' && +t.value > 0) toast(`Tope de FPS ${t.value}: se escribe fps_max en framerush.cfg.`);
    if (id === 'resolution-override' && t.value === 'custom') toast('Resolución personalizada: control pendiente de definir.');
    if (id === 'profile') toast(`Perfil «${t.selectedOptions[0].text}» cargado.`);
    if (id === 'language') toast(t.value === 'en' ? 'Language: English (vista previa; i18n pendiente)' : 'Idioma: español');
    if (id === 'dry-run') setText('[data-setting="apply"] .lbl', t.checked ? 'Simular optimización' : 'Aplicar optimización');
    if (id === 'auto-revert-on-exit') sess();
    if (id === 'thermal-alerts' || id === 'thermal-alert-threshold') S.alerted = false;
  });

  document.addEventListener('keydown', e => { if (e.ctrlKey && e.key === 'Enter') { e.preventDefault(); apply(); } }); /* shortcut-apply */

  /* ---------- Arranque ---------- */
  function elevateButton() {
    DS('run-as-admin').forEach(b => { b.disabled = S.admin; b.textContent = S.admin ? '✓ Helper elevado activo' : ELEVATE_LABEL[D.os]; });
  }

  async function init(sampleKey) {
    stopStreams.forEach(f => f()); stopStreams = [];
    D = await api.load(sampleKey);
    Object.assign(S, { admin: D.elevated, game: Math.max(0, D.games.findIndex(g => g.running)), hist: [], rt: false, alerted: false, rb: [], turbo: false, fps: [], lastSensors: null, chk: [], stc: D.steamRecs.map(s => !!s.checked) });
    $$('[data-touched]').forEach(x => { delete x.dataset.touched; });
    $$('.devbar [data-os]').forEach(b => b.classList.toggle('on', b.dataset.os === D.os));

    DS('game-target').forEach(s => {
      s.innerHTML = D.games.map((g, i) => `<option value="${i}">${esc(g.name)}${g.running ? ' (en ejecución)' : ''}${g.antiCheat ? ' · ' + esc(g.antiCheat.name) : ''}</option>`).join('');
      s.value = String(S.game);
    });
    DS('profile').forEach(s => { s.innerHTML = D.profiles.map(p => `<option value="${esc(p.id)}">${esc(p.name)}</option>`).join('') + '<option value="nuevo">+ Nuevo…</option>'; });
    DS('restore-after-crash').forEach(e => { e.hidden = !D.pendingSession; });
    setText('.crashN', D.pendingSession ? D.pendingSession.changes : '');
    setText('.rtApi', RT_API[D.os] || '');
    DS('compat-warning').forEach(e => { e.hidden = true; }); rebootBanner();
    elevateButton();
    setLevel(DEFAULT_LEVEL); sess(); renderPerf();
    stopStreams.push(api.onSensors(D, onSensors), api.onFps(D, onFps));
  }

  if (!api.inTauri) {
    document.body.classList.add('preview');
    DS('platform-preview').forEach(e => { e.hidden = false; });
  }
  init('win');
})();

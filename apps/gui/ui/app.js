/*
 * FrameRush GUI (diseño final: design/v3-overlay-compact.html).
 * Ids de data-setting, valores y defaults: design/SETTINGS.md. Reglas de comportamiento: SETTINGS.md §4 y §6.
 * Todo lo que depende del equipo llega en el snapshot de FrameRushApi.load() (R8); aquí solo hay
 * textos de la interfaz y reglas del plan. Lo que el snapshot no trae se oculta o se muestra
 * como "No disponible": nunca se rellena.
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
  const CAT = { telemetry: 'Telemetría (envío de datos)', updater: 'Actualizadores', sync: 'Sincronización y nube', launcher: 'Lanzadores de juegos', browser: 'Navegadores y apps', overlay: 'Superposiciones (overlays)', peripheral: 'Periféricos', service_a: 'Servicios · grupo A (desde Agresivo)', service_b: 'Servicios · grupo B (solo Extremo)' };
  const ACT = { lower_priority: 'Bajar prioridad', suspend: 'Pausar', kill: 'Cerrar', stop_service: 'Detener servicio' };
  const RISK = { low: ['Bajo', 1], medium: ['Medio', 2], high: ['Alto', 3] };
  const LV = { normal: 1, agresivo: 2, extremo: 3 };
  const LV_NAME = ['', 'Normal', 'Agresivo', 'Extremo'];
  const LV_NOTE = ['',
    'Normal (sin administrador): baja la prioridad de lo que no hace falta, pausa actualizadores y telemetría y aplica ajustes de tu usuario (Game DVR, notificaciones, efectos visuales). No cambia la energía.',
    'Agresivo (requiere administrador, sin reiniciar): lo de Normal + pausa sincronización, lanzadores y periféricos; servicios del grupo A; prioridad Alta siempre; plan «FrameRush Max»; pantalla a su frecuencia máxima; configuración del juego bloqueada.',
    '⚠ Extremo (requiere reiniciar): lo de Agresivo + cierra procesos de riesgo bajo, servicios del grupo B, HAGS y desactiva VBS/HVCI y C-states. Reservar núcleos y el temporizador siguen siendo opcionales.'];
  const DEFAULT_LEVEL = 2; /* Agresivo (PLAN §3, valores por defecto) */
  /* Ajustes que requieren reinicio (SETTINGS §4.7). */
  const REBOOT = { hags: 'HAGS', 'vbs-hvci-off': 'VBS/HVCI desactivado', 'cstates-off': 'C-states desactivados', 'timer-resolution': 'temporizador global' };
  /* run-as-admin: W y L según SETTINGS.md; la etiqueta de macOS no está definida (pendiente). */
  const ELEVATE_LABEL = { win: '🔒 Ejecutar como administrador', lin: '🔒 Pedir permisos de administrador (polkit)', mac: '🔒 Pedir permisos de administrador' };
  const RT_API = { lin: ' (SCHED_RR)' };
  const FPS_SOURCE = { presentmon: 'PresentMon (ETW)', mangohud: 'Logs de MangoHud' };
  const GPU_NOTE = { write: 'Se guarda en la configuración de gráficos de Windows (UserGpuPreferences)', info: 'En Linux solo es informativo (DRI_PRIME / prime-run)' };
  const POWER_BACKEND = { win: 'Plan de energía de Windows', lin: 'power-profiles-daemon', mac: 'pmset' };
  /* Motivo de cada protegido (core::ProtectedReason). */
  const PROT_WHY = { os_critical: 'parte esencial del sistema', compositor: 'dibuja el escritorio (compositor)', audio: 'sonido', gpu_driver: 'controlador de la GPU', anti_cheat: 'anticheat', self_app: 'FrameRush' };
  /* Ajustes con backend: se ocultan si el snapshot no los trae en `available` (P15). */
  const BACKEND_IDS = ['game-dvr-off', 'windows-game-mode', 'notifications-off', 'visual-effects-performance', 'uwp-background-off',
    'scheduler-foreground-boost', 'power-throttling-off', 'mmcss-games', 'defender-exclusions', 'hags', 'vbs-hvci-off', 'cstates-off',
    'memory-trim', 'standby-purge', 'custom-power-plan', 'refresh-rate-max', 'game-priority-persistent', 'fullscreen-optimizations-off',
    'gpu-preference', 'cpu-affinity', 'timer-resolution'];
  /* Lo que la app aún no hace: se dice en vez de simularlo. */
  const NOT_WIRED = 'aún no funciona en esta versión; no se ha cambiado nada.';
  const NO_GAME = { name: 'Ningún juego detectado', exe: '', appid: '', pids: [] };
  const GB = 1024 ** 3, MB = 1024 ** 2;

  /* ---------- Estado ---------- */
  let D = null;           /* snapshot del backend */
  let stopStreams = [];
  const S = { level: DEFAULT_LEVEL, admin: false, game: 0, plan: null, hist: [], watching: false, profiles: {}, base: null, rt: false, alerted: false, rb: [], turbo: false, fps: [], chk: [], stc: [], fanIds: '', protOpen: false, released: [], turboPending: null };

  const game = () => D.games[S.game] || { ...NO_GAME, exe: D.gamesNote || '' };
  /* Anticheat del juego o, sin adaptador, los que están en ejecución (cuentan para cualquier juego, It1.3). */
  const antiCheat = () => game().antiCheat || (D.anticheats.length ? { name: D.anticheats.join(', '), process: D.anticheats.join(', ') } : null);
  /* SETTINGS §4.2: kill solo en Extremo y con riesgo bajo; si no, se suspende. */
  const effAct = p => (p.action === 'kill' && (S.level < 3 || p.risk !== 'low') ? 'suspend' : p.action);
  /* SETTINGS §4.3 y P12: lo que pide admin lleva candado mientras no haya helper elevado. */
  const locked = p => p.locked && !S.admin;
  const fmt = bytes => (bytes >= GB ? (bytes / GB).toFixed(1) + ' GB' : Math.round(bytes / MB) + ' MB');
  const gb = bytes => `${Math.round(bytes / GB * 10) / 10} GB`;
  const avail = id => D.available.includes(id);

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
    return `<span class="act" data-setting="process-action" title="${fell ? 'Cerrar solo en Extremo y con riesgo bajo: se pausa en su lugar' : ACT[a]}">${ACT[a]}${fell ? ' *' : ''}</span>`;
  };
  const lockBadge = p => (locked(p) ? ' <span class="adm" data-setting="admin-lock">🔒 Requiere administrador</span>' : '');
  const riskBadge = p => { const r = RISK[p.risk]; return `<span class="badge r${r[1]}">${r[0]}</span>`; };
  /* Reanudar / volver a limitar una fila sin tocar el resto de la sesión. */
  const rowBtn = p => (p.limited
    ? ` <button class="btn rowAct" data-release="${esc(p.id)}" data-setting="process-release" title="Reanuda solo estos procesos; el resto sigue limitado">▶ Reanudar</button>`
    : S.released.includes(p.id) ? ` <button class="btn rowAct" data-limit="${esc(p.id)}" data-setting="process-release" title="Vuelve a limitar estos procesos (nunca los cierra)">⏸ Limitar</button>` : '');
  const rowHTML = (p, i) => `<label class="p prow ${S.chk[i] ? '' : 'off'} ${locked(p) ? 'lk' : ''}" title="${esc(p.desc)}"><input type="checkbox" data-i="${i}" ${S.chk[i] ? 'checked' : ''} ${locked(p) ? 'disabled' : ''}><span class="n"><b>${esc(p.name)}</b>${lockBadge(p)}${p.limited ? ' <span class="badge r2">limitado</span>' : ''}<i>${esc(p.desc)}</i></span><span class="r">${actBadge(p)} ${p.ramBytes ? fmt(p.ramBytes) : '—'} · ${p.cpu == null ? '— CPU' : p.cpu.toFixed(1) + '%'}${riskBadge(p)}${rowBtn(p)}</span></label>`;
  const catHTML = c => `<div class="cat">${CAT[c]}<span class="sp"></span><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="1">todos</button><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="0">ninguno</button></div>`;
  const lockHTML = l => `<div class="p lk" data-setting="protected-processes"><span>🔒</span><span class="n"><b>${esc(l.name)}</b><i>${esc(l.why)} · FrameRush nunca lo toca</i></span><span class="r"><span class="badge r0">Protegido</span></span></div>`;
  const stHTML = (s, i, dis) => `<label class="p ${dis ? 'dis' : ''}"><input type="checkbox" data-s="${i}" ${S.stc[i] ? 'checked' : ''} ${dis ? 'disabled' : ''}><span class="n"><b>${esc(s.name)}</b><span class="src">${esc(s.source)}</span><i>${esc(s.why)}${dis ? ' · requiere overlay desactivado' : ''}</i></span><span></span></label>`;

  /* ---------- Nivel (SETTINGS §4.1) ---------- */
  function setLevel(l) {
    S.level = l;
    /* Lo que viene marcado lo decide el plan de core para cada nivel. */
    D.processes.forEach((p, i) => { S.chk[i] = p.checked[l - 1] && !locked(p); });
    $$('[data-l]').forEach(b => b.classList.toggle('on', +b.dataset.l === l));
    $$('.lvNote').forEach(n => { n.textContent = LV_NOTE[l]; n.dataset.lv = l; });
    const pw = D.device.power;
    S.plan = pw ? pw.forLevel[l - 1] : null;
    $$('.row[data-min]').forEach(r => {
      const ok = LV[r.dataset.min] <= l, x = r.querySelector('input');
      r.classList.toggle('na-lv', !ok);
      if (x) { x.disabled = !ok; x.checked = ok && !('optin' in r.dataset); }
      const s = r.querySelector('small');
      if (s && !ok && !s.dataset.o) s.dataset.o = s.textContent;
      if (s && s.dataset.o) s.textContent = ok ? s.dataset.o : 'Disponible desde ' + LV_NAME[LV[r.dataset.min]];
    });
    const ac = antiCheat();
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
    if (!D.processes.length) h += '<p class="note">Ahora mismo no hay abierto ningún programa que FrameRush sepa optimizar.</p>';
    /* Solo informativos: plegados por defecto; se recuerda si el usuario los abre (render rehace la lista). */
    const prot = $('#rows details.prot');
    if (prot) S.protOpen = prot.open;
    if (D.protected.length) {
      h += `<details class="adv prot" ${S.protOpen ? 'open' : ''}><summary>Protegidos abiertos: FrameRush nunca los toca · ${D.protected.length}</summary>`;
      D.protected.forEach(l => { h += lockHTML({ name: l.name, why: PROT_WHY[l.reason] || l.reason }); });
      h += '</details>';
    }
    $('#rows').innerHTML = h;
    D.steamRecs.forEach((s, i) => { if (s.dependsOn != null && !S.stc[s.dependsOn]) S.stc[i] = false; });
    $('#steamList').innerHTML = D.steamRecs.map((s, i) => stHTML(s, i, s.dependsOn != null && !S.stc[s.dependsOn])).join('');
    summary();
  }

  function summary() {
    let n = 0, ram = 0, cpu = 0;
    D.processes.forEach((p, i) => { if (S.chk[i]) { n++; ram += p.ramBytes; cpu += p.cpu || 0; } });
    /* impact-estimate: solo RAM y CPU, nunca cifras de FPS (PLAN §6). */
    DS('impact-estimate').forEach(e => { e.innerHTML = `Se liberarían aprox. <b>${fmt(ram)}</b> de RAM y <b>${cpu.toFixed(1)} %</b> de CPU · ${n} marcados`; });
    setText('.pm', `${n}/${D.processes.length} marcados`);
    setText('.stm', `${S.stc.filter(Boolean).length}/${D.steamRecs.length} recomendaciones`);
  }

  /* ---------- Energía (P3) ---------- */
  function renderPower() {
    const o = D.device.power;
    if (!o) {
      DS('power-plan').forEach(g => { g.innerHTML = ''; });
      setText('.pwrCur', 'No disponible'); setText('.pwrNote', 'No se pudieron leer los planes de energía de este equipo.');
      return;
    }
    DS('power-plan').forEach(g => {
      g.innerHTML = o.plans.map((x, i) => `<button data-i="${i}" class="${i === S.plan ? 'on' : ''}" title="${i === o.current ? 'Plan actual del sistema' : ''}">${esc(x)}${i === o.current ? ' •' : ''}</button>`).join('');
    });
    setText('.pwrCur', o.current == null ? 'plan actual desconocido' : o.plans[o.current]);
    const custom = D.caps.windows && active('custom-power-plan');
    const what = custom ? `se activará «FrameRush Max» (base: ${o.plans[S.plan] ?? 'plan actual'})`
      : S.plan == null || S.plan === o.current ? 'sin cambios en este nivel' : `se cambiará a «${o.plans[S.plan]}» al aplicar`;
    setText('.pwrNote', `${POWER_BACKEND[D.os]} · ${what} · se deshace con Restaurar`);
  }

  /* ---------- Capacidades del SO y juego seleccionado ---------- */
  function caps() {
    const k = D.caps, g = game(), ac = !!antiCheat();
    const capOf = { win: k.windows, affinity: k.affinity, timer: k.timer, gpu: k.gpu };
    $$('[data-cap]').forEach(e => { e.hidden = !capOf[e.dataset.cap]; });
    DS('gpu-preference').forEach(s => { s.disabled = k.gpu === 'info'; });
    setText('.gpuNote', GPU_NOTE[k.gpu] || '');
    /* SETTINGS §4.4: el anticheat desactiva afinidad y timer. */
    ['cpu-affinity', 'timer-resolution'].forEach(id => DS(id).forEach(x => { x.disabled = ac; if (ac) x.checked = false; }));
    DS('anti-cheat-warning').forEach(e => {
      e.hidden = !ac;
      e.textContent = ac ? `🛡 Anticheat abierto (${antiCheat().name}): se desactivan «Reservar núcleos» y el temporizador global; el juego solo recibe más prioridad y el anticheat no se toca.` : '';
    });
    setText('.fpsNote', k.fps ? FPS_SOURCE[k.fps] : 'sin medición de FPS todavía');
    setText('.gameName', g.name); setText('.gameExe', g.exe); setText('.gameId', g.appid);
    setText('.gamePid', g.pids.length ? ` · PID ${g.pids.join(', ')}` : '');
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
    const d = D.device, cpu = d.cpu, ac = antiCheat();
    DS('battery-warning').forEach(e => { e.hidden = !d.batteryWarning; });
    BACKEND_IDS.forEach(id => hideRow(id, !avail(id)));
    hideRow('hags', !avail('hags') || d.hagsSupported !== true);
    hideRow('vbs-hvci-off', !avail('vbs-hvci-off') || d.vbsActive !== true);
    hideRow('timer-resolution', !avail('timer-resolution') || d.windows11 !== true);
    const slow = d.displays.filter(x => x.maxHz != null && x.maxHz > x.hz);
    setText('.hzNote', slow.map(x => `${x.width}×${x.height}: ${x.hz} Hz → ${x.maxHz} Hz`).join(' · '));

    /* hardware-profile: solo lo detectado */
    const cores = !cpu ? '' : cpu.cores.length
      ? (cpu.cores.some(c => c.efficiency) ? `${cpu.cores.filter(c => !c.efficiency).length}P + ${cpu.cores.filter(c => c.efficiency).length}E` : `${cpu.cores.length} núcleos físicos`) + ` / ${cpu.threads} hilos`
      : `${cpu.threads} hilos`;
    const gpus = d.gpus.map(x => {
      const n = esc(x.name) + (x.vramBytes ? ` ${gb(x.vramBytes)}` : '');
      return x.preferred ? `<b>${n}</b>` : n;
    }).join(' + ');
    DS('hardware-profile').forEach(e => {
      e.innerHTML = [cpu ? `<b>${esc(cpu.model)}</b> · ${cores}` : 'CPU: No disponible', gpus || 'GPU: No disponible', d.manufacturer ? esc(d.manufacturer) : '']
        .filter(Boolean).join(' · ');
    });

    /* gpu-preference: la GPU que elige core (más VRAM dedicada) o dejarlo al sistema */
    DS('gpu-preference').forEach(sel => {
      sel.innerHTML = d.gpus.filter(x => x.preferred).map(x => `<option value="alto">${esc(x.name)}${x.vramBytes ? ` ${gb(x.vramBytes)}` : ''}</option>`).join('') +
        '<option value="sistema">La que elija el sistema (sin cambios)</option>';
    });

    /* resolution-override: resolución actual del primer monitor */
    DS('resolution-override').forEach(sel => {
      const m = d.displays[0];
      sel.innerHTML = m ? `<option value="nativa">Nativa (${m.width}×${m.height})</option>` : '<option value="nativa">Nativa</option>';
    });

    const ka = first('keep-alive-list');
    if (ka && !ka.dataset.touched) ka.value = d.keepAlive.value.join(', ');

    /* Umbrales térmicos: core::defaults (min(TjMax − 10, slowdown GPU − 5); aviso = alerta − 15) */
    const al = first('thermal-alert-threshold'), wa = first('thermal-warn-threshold');
    if (al && !al.dataset.touched) al.value = Math.round(d.thermalAlert.value);
    if (wa && !wa.dataset.touched) wa.value = Math.round(d.thermalWarn.value);
    const slowdown = d.gpus.find(x => x.slowdown != null)?.slowdown ?? null;
    setText('.thrNote', d.thermalAlert.estimated ? 'Valores estimados: no se pudo leer la temperatura máxima de la CPU (TjMax) ni la de la GPU.'
      : `Calculado: TjMax ${cpu?.tjmax ?? '—'} °C − 10 · límite de la GPU ${slowdown ?? '—'} °C − 5 → ${Math.round(d.thermalAlert.value)} °C`);

    /* manual-recommendations: todavía no hay reglas de recomendaciones manuales */
    DS('manual-recommendations').forEach(e => { e.innerHTML = ''; e.closest('details').hidden = true; });

    /* compat-warning: solo lo que está instalado */
    $$('.cwList').forEach(e => {
      e.innerHTML = (d.virtualization.length ? d.virtualization.map(v => `<li>${esc(v)} (instalado)</li>`).join('') : '<li>No se detectó ningún programa instalado que use el hipervisor</li>') +
        (ac && ac.mayRequireVbs ? `<li>${esc(ac.name)} puede exigir VBS/HVCI: la opción queda bloqueada</li>` : ac ? `<li>Anticheat ${esc(ac.name)}: comprueba que no exija VBS</li>` : '');
    });

    setText('.affNote', d.affinityWarning ? `⚠ Con ${cpu.cores.length} núcleos físicos, reservar núcleos a mano suele empeorar el rendimiento` : 'Opcional en todos los niveles');
    renderAffinity();
    if (S.lastSensors) onSensors(S.lastSensors);
  }

  /* cpu-affinity-cores: la topología real; marcadas las que propone core::defaults::affinity_cpus */
  function renderAffinity() {
    const b = first('cpu-affinity-cores'); if (!b) return;
    const cpu = D.device.cpu, on = new Set(D.device.affinity.value);
    const cpus = cpu ? cpu.cores.flatMap((c, k) => c.logical.map(i => ({ i, k, e: c.efficiency }))) : [];
    b.innerHTML = cpus.sort((x, y) => x.i - y.i).map(({ i, k, e }) =>
      `<input type="checkbox" id="core${i}" ${on.has(i) ? 'checked' : ''}><label for="core${i}" title="Núcleo físico ${k} · CPU lógica ${i}">${e ? 'E' : 'P'}${i}</label>`).join('');
    syncAffinityCores();
  }
  function syncAffinityCores() {
    const aff = first('cpu-affinity'), ac = !!antiCheat();
    $$('[data-setting="cpu-affinity-cores"] input').forEach(x => { x.disabled = ac || !(aff && aff.checked); });
  }

  /* ---------- Rendimiento (antes/después) ---------- */
  /* Sin medición de FPS no hay antes/después: el gráfico queda vacío. */
  function renderPerf() {
    $('#perfBefore').setAttribute('points', '');
    $('#perfAfter').setAttribute('points', '');
    setText('.perfSum', 'aún no se miden los FPS');
  }

  /* ---------- Sesión ---------- */
  function rebootBanner() {
    DS('reboot-required').forEach(e => { e.hidden = !S.rb.length; const l = e.querySelector('.rbList'); if (l) l.textContent = S.rb.map(x => REBOOT[x]).join(', '); });
  }

  /* Lo seleccionado en la UI; lo aplicado de verdad lo da `framerush status` hasta que la app aplique. */
  function statusText() {
    const pw = D.device.power, g = game();
    return [
      `Nivel         : ${LV_NAME[S.level]}`,
      `Procesos      : ${S.chk.filter(Boolean).length} marcados`,
      `Energía       : ${pw && S.plan != null ? pw.plans[S.plan] : 'sin cambios'}${active('custom-power-plan') ? ' + FrameRush Max' : ''}`,
      `Prioridad     : ${S.rt ? 'Tiempo real' : priorityValue() === 'normal' ? 'Normal' : 'Alta'}${active('game-priority-persistent') ? ' (siempre, hasta Restaurar)' : ''}`,
      `Refresco      : ${active('refresh-rate-max') ? 'máximo' : 'sin cambios'}`,
      `Juego         : ${g.name}`,
      `No pausar     : ${first('keep-alive-list')?.value || '—'}`,
      `Sesión previa : ${D.pendingSession ? D.pendingSession.changes + ' cambios sin restaurar' : 'no'}`,
    ].join('\n');
  }

  function sess() {
    const on = S.hist.length > 0;
    DS('session-status').forEach(e => {
      e.classList.toggle('on', on);
      const t = e.querySelector('.sessT');
      if (t) t.textContent = on ? `Sesión activa · ${S.hist.reduce((a, b) => a + b, 0)} cambios · ${S.hist.length} aplicación(es)${S.watching ? ' · se deshace al cerrar el juego' : ''}` : 'Sin cambios activos';
    });
    DS('restore').forEach(b => { b.disabled = !on && !D.pendingSession; });
    DS('undo-last').forEach(b => { b.disabled = !on; });
  }

  const levelToggles = () => $$('.tg[data-setting]').filter(x => x.checked && !x.disabled && shown(x) && x.closest('[data-min]'));

  /* Informe de apply/undo/revert: hechos, fallidos y lo que no se pudo (sin inventar nada). */
  function report(verb, r) {
    const fails = r.lines.filter(l => l.outcome === 'failed');
    let msg = `${verb}: ${r.done} hecho(s)${r.failed ? ` · ${r.failed} fallido(s)` : ''}.`;
    if (fails.length) msg += ` Primer fallo: ${fails[0].what} (${fails[0].detail}).`;
    if (r.needsAdmin.length) msg += ` Necesitan administrador (pulsa «${ELEVATE_LABEL[D.os].replace(/^\S+ /, '')}»): ${r.needsAdmin.join(', ')}.`;
    if (r.notWired.length) msg += ` Aún no funciona en esta versión, no se aplicó: ${r.notWired.join(', ')}.`;
    if (r.notes.length) msg += ` No aplicado: ${r.notes.join('; ')}.`;
    if (!r.lines.length && !r.notWired.length && !r.needsAdmin.length && !r.notes.length) msg = `${verb}: no había nada que hacer.`;
    toast(msg);
  }

  async function run(verb, op) {
    DS('apply').forEach(b => { b.disabled = true; });
    try {
      const r = await op();
      report(verb, r);
      return r;
    } catch (err) {
      toast(`${verb}: ${err && err.message ? err.message : err}`);
      return null;
    } finally {
      DS('apply').forEach(b => { b.disabled = false; });
    }
  }

  /* El snapshot se carga una vez: tras cada acción se pregunta qué filas quedaron limitadas. */
  async function syncLimited() {
    try {
      const ids = await api.limitedRows();
      D.processes.forEach(p => { p.limited = ids.includes(p.id); });
      S.released = S.released.filter(id => !ids.includes(id));
      render();
    } catch (err) { toast(`No se pudo leer qué procesos están limitados: ${err && err.message ? err.message : err}`); }
  }

  async function releaseRow(id) {
    const r = await run('Reanudar', () => api.releaseRow(id));
    if (!r) return;
    if (!r.failed) S.released.push(id);
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; }
    sess(); syncLimited();
  }

  async function limitRow(id) {
    const r = await run('Limitar', () => api.limitRow(id, S.level, keepAliveList()));
    if (!r) return;
    if (r.done) S.hist.push(r.done);
    sess(); syncLimited();
  }

  async function undo() {
    const r = await run('Deshacer', api.undo);
    if (!r) return;
    S.hist.pop();
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; }
    sess(); syncLimited();
  }

  async function revert() {
    const r = await run('Restaurar', api.revert);
    if (!r) return;
    S.watching = false;
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; DS('restore-after-crash').forEach(x => { x.hidden = true; }); }
    S.released = [];
    sess(); syncLimited();
  }

  /* El backend vuelve a planificar con el equipo de ahora y aplica solo lo marcado. */
  async function apply(force, verb = 'Aplicar', turbo = false) {
    const n = S.chk.filter(Boolean).length + S.stc.filter(Boolean).length + levelToggles().length;
    if (first('dry-run')?.checked) { toast(`Simulación: se aplicarían ${n} cambios. No se ha modificado nada.`); return; }
    if (!force && active('vbs-hvci-off')) { /* SETTINGS §4.8 */
      /* La confirmación vuelve a aplicar igual: con Turbo si esto era Turbo. */
      S.cwRetry = { verb, turbo };
      DS('compat-warning').forEach(e => { e.hidden = false; });
      toast('Lee y confirma el aviso de compatibilidad de arriba para aplicar.');
      return;
    }
    const r = await run(verb, () => api.apply({
      level: S.level,
      rows: D.processes.filter((p, i) => S.chk[i]).map(p => p.id),
      settings: levelToggles().map(x => x.dataset.setting),
      powerPlan: S.plan ?? null,
      game: gameSelection(),
      autoRevert: !!first('auto-revert-on-exit')?.checked,
      turbo: turbo ? { rescanSecs: +(first('turbo-rescan')?.value || 60) } : null,
      keepAlive: keepAliveList(),
    }));
    if (r) S.watching = !!r.watching;
    if (r) S.turbo = turbo && !!r.watching;
    if (S.turbo) S.turboPending = null; /* ya vigila el juego: no hace falta el reintento */
    if (r && r.done) S.hist.push(r.done);
    sess(); syncLimited();
  }

  /*
   * keep-alive-list (P7) tal como está en el campo: arranca con la del equipo (R8) y el usuario
   * puede cambiarla. Vacía = no se protege ninguna app; sin el campo, null (la del equipo).
   */
  function keepAliveList() {
    const x = first('keep-alive-list');
    return x ? x.value.split(',').map(s => s.trim()).filter(Boolean) : null;
  }

  /* Panel del juego (R3): lo que el backend necesita para el GameBoost. FSO e IFEO van en settings. */
  function gameSelection() {
    const g = D.games[S.game];
    if (!g) return null;
    const aff = first('cpu-affinity');
    const cores = $$('[data-setting="cpu-affinity-cores"] input:checked').map(x => +x.id.replace('core', ''));
    const affOn = aff && aff.checked && !aff.disabled && shown(aff) && cores.length;
    const gpu = first('gpu-preference');
    const on = id => { const x = first(id); return !!x && x.checked && !x.disabled && shown(x); };
    return {
      appid: g.appid,
      priority: priorityValue() || 'alta',
      realtimeConfirmed: S.rt,
      affinity: affOn ? cores : null,
      highPerformanceGpu: !!(gpu && !gpu.disabled && shown(gpu) && gpu.value === 'alto'),
      /* Panel del adaptador (It5.1); game-config-lock va en settings con el nivel. */
      adapter: g.adapter ? {
        tune: on('game-config-tune'),
        fpsCap: Math.max(0, Math.round(+(first('fps-cap')?.value || 0))),
        bench: on('bench-cfg'),
      } : null,
    };
  }

  /* Watchdog (It1.4, P5): el juego se cerró y el backend revirtió lo de sesión. */
  function onGameExited(e) {
    S.watching = false;
    if (D.games[S.game]) D.games[S.game].running = false;
    const resumedOnly = S.turbo && !first('auto-revert-on-exit')?.checked;
    S.turbo = false;
    if (e.error) toast(`Juego cerrado, pero no se pudieron deshacer los cambios: ${e.error}`);
    else {
      report(resumedOnly ? 'Juego cerrado, Turbo reanudó lo pausado' : 'Juego cerrado, cambios temporales deshechos', e.outcomes);
      if (!e.outcomes.sessionOpen) { S.hist = []; D.pendingSession = null; }
    }
    S.released = []; sess(); syncLimited();
  }

  /*
   * Lanzamiento fuera de la app (It3.2, P4): con profile-auto-apply y un perfil guardado, se elige el
   * juego, se carga su perfil y se aplica como con el botón (las confirmaciones siguen pidiéndose).
   */
  function onGameLaunched(e) {
    const i = D.games.findIndex(g => g.appid === e.appid);
    if (i < 0) return; /* instalado después de abrir la app: no hay datos de él */
    Object.assign(D.games[i], { running: true, pids: e.pids });
    /* Lanzado con «Jugar ahora»: Turbo aplica el perfil una sola vez (P4). */
    if (S.turboPending === e.appid) { const first = !S.turboRetried; S.turboPending = null; S.turboRetried = true; startTurbo(i, first); return; }
    const p = S.profiles[e.appid];
    if (!p || !first('profile-auto-apply')?.checked) {
      if (S.game === i) caps();
      toast(`${D.games[i].name} se ha iniciado${p ? ' (auto-aplicar desactivado)' : ' · sin perfil guardado'}.`);
      return;
    }
    S.game = i;
    DS('game-target').forEach(s => { s.value = String(i); });
    caps();
    useProfile(e.appid, true);
    apply(false, `${p.name} iniciado, perfil aplicado`);
  }

  async function copyLaunchOptions() {
    const a = game().adapter;
    if (!a) return;
    try {
      await navigator.clipboard.writeText(a.launchOptions);
      toast(`Copiado al portapapeles: ${a.launchOptions} (opciones de lanzamiento de ${game().name} en Steam).`);
    } catch (err) {
      toast(`No se pudo copiar (${err && err.message ? err.message : err}): ${a.launchOptions}`);
    }
  }

  /*
   * «Jugar ahora» (It5.2): lanza el juego con Steam y, cuando arranca (evento game-launched),
   * aplica su perfil con Turbo: congela lo marcado (cerrar pasa a congelar), re-escanea cada
   * turbo-rescan y al salir reanuda siempre lo congelado (P5). Si ya está abierto, Turbo se
   * aplica sin lanzarlo. En simulación (P11) se lanza sin congelar nada.
   */
  async function launchGame() {
    const g = D.games[S.game];
    if (!g) { toast('Elige un juego de Steam en «Juego».'); return; }
    if (!api.inTauri) { toast('Jugar ahora solo funciona dentro de la app.'); return; }
    const dry = !!first('dry-run')?.checked;
    /* Se marca antes de lanzar: game-launched puede llegar antes de que vuelva la llamada. */
    S.turboPending = dry ? null : g.appid;
    S.turboRetried = false;
    let running;
    try {
      running = await api.launchGame(g.appid);
    } catch (err) {
      S.turboPending = null;
      toast(`No se pudo lanzar ${g.name}: ${err && err.message ? err.message : err}`);
      return;
    }
    g.running = running;
    if (running) {
      S.turboPending = null;
      if (dry) toast(`Simulación: ${g.name} ya está abierto; Turbo no pausaría nada en este modo.`);
      else startTurbo(S.game);
      return;
    }
    toast(dry
      ? `Simulación: lanzando ${g.name} sin pausar nada (${S.chk.filter(Boolean).length} fila(s) marcada(s)).`
      : `Lanzando ${g.name} con Steam… Turbo se aplica en cuanto arranque.`);
  }

  /*
   * retry: algunos juegos (CS2) arrancan un primer proceso que se cierra enseguida y luego el
   * definitivo. Si al aplicar el juego ya no estaba (sin watchdog), Turbo queda pendiente para
   * el siguiente arranque, una sola vez.
   */
  async function startTurbo(i, retry = false) {
    const g = D.games[i];
    S.game = i;
    DS('game-target').forEach(s => { s.value = String(i); });
    caps();
    /* P4: con el perfil del juego ya elegido no se recarga, para no perder lo marcado después a mano. */
    if (S.profiles[g.appid] && first('profile')?.value !== String(g.appid)) useProfile(g.appid, true);
    await apply(false, `Turbo · ${g.name}`, true);
    if (!S.turbo && retry) S.turboPending = g.appid;
  }

  /* Re-escaneo de Turbo: solo avisa si congeló algo nuevo. */
  function onTurboRescan(e) {
    if (e.error) { toast(`Turbo: no se pudo re-escanear (${e.error}).`); return; }
    if (e.outcomes && e.outcomes.done) { toast(`Turbo: ${e.outcomes.done} programa(s) nuevo(s) en pausa.`); syncLimited(); }
  }

  /* ---------- Perfiles por juego (It3.2) ---------- */
  /* No van en el perfil: la clave (juego), el propio perfil, ajustes de la app y lo que se guarda aparte. */
  const PROFILE_SKIP = new Set(['profile', 'profile-auto-apply', 'game-target', 'language', 'perf-metric', 'power-plan', 'game-priority']);

  /* Todo lo elegible de la UI salvo lo de solo lectura (SETTINGS.md, profile-save). */
  function profileState() {
    const settings = {};
    $$('input[data-setting], select[data-setting]').forEach(x => {
      const id = x.dataset.setting;
      if (PROFILE_SKIP.has(id) || id in settings) return;
      settings[id] = x.type === 'checkbox' ? x.checked : x.value;
    });
    const pw = D.device.power;
    return {
      level: S.level,
      rows: Object.fromEntries(D.processes.map((p, i) => [p.id, !!S.chk[i]])),
      steam: Object.fromEntries(D.steamRecs.map((s, i) => [s.name, !!S.stc[i]])),
      settings,
      /* El plan por nombre: los índices cambian si se crean o borran planes (R8). */
      powerPlan: pw && S.plan != null ? pw.plans[S.plan] : null,
      /* Tiempo real nunca se guarda (invariante): queda en Alta. */
      priority: S.rt ? 'alta' : priorityValue() || 'alta',
      affinityCores: $$('[data-setting="cpu-affinity-cores"] input:checked').map(x => +x.id.replace('core', '')),
    };
  }

  /* Carga un perfil sobre los valores del nivel; lo que este equipo no tiene ahora se queda como está. */
  function loadProfile(p) {
    setLevel(LV_NAME[p.level] ? p.level : DEFAULT_LEVEL);
    D.processes.forEach((r, i) => { if (r.id in (p.rows || {}) && !locked(r)) S.chk[i] = !!p.rows[r.id]; });
    D.steamRecs.forEach((s, i) => { if (s.name in (p.steam || {})) S.stc[i] = !!p.steam[s.name]; });
    for (const [id, v] of Object.entries(p.settings || {})) {
      if (PROFILE_SKIP.has(id)) continue;
      DS(id).forEach(x => {
        if (x.disabled) return; /* nivel insuficiente, anticheat o sin backend */
        if (x.type === 'checkbox') x.checked = !!v;
        else if (x.tagName !== 'SELECT' || [...x.options].some(o => o.value === v)) { x.value = v; x.dataset.touched = 1; }
      });
    }
    const pw = D.device.power, plan = pw && p.powerPlan != null ? pw.plans.indexOf(p.powerPlan) : -1;
    if (plan >= 0) S.plan = plan;
    S.rt = false; $('#rtBox').hidden = true;
    $$('[data-setting="game-priority"] button').forEach(x => x.classList.toggle('on', x.dataset.v === (p.priority || 'alta')));
    if (p.affinityCores) {
      const on = new Set(p.affinityCores);
      $$('[data-setting="cpu-affinity-cores"] input').forEach(x => { x.checked = on.has(+x.id.replace('core', '')); });
    }
    /* Las mismas reglas que al tocar a mano: P10 y P1. */
    if (antiCheat()) setCheck('vbs-hvci-off', false);
    if (priorityValue() === 'normal') setCheck('game-priority-persistent', false);
    setText('[data-setting="apply"] .lbl', first('dry-run')?.checked ? 'Simular optimización' : 'Aplicar optimización');
    syncAffinityCores(); renderPower(); render(); sess();
  }

  function fillProfiles(selected) {
    DS('profile').forEach(s => {
      s.innerHTML = '<option value="global">Global</option>' +
        Object.entries(S.profiles).map(([id, p]) => `<option value="${esc(id)}">${esc(p.name)}</option>`).join('') +
        (D.games.length ? '<option value="nuevo">Nuevo: guardar el juego elegido…</option>' : '');
      s.value = selected in S.profiles ? String(selected) : 'global';
    });
  }

  /* El perfil del juego elegido, o Global (los valores del nivel por defecto, como al abrir). */
  function useProfile(appid, quiet) {
    const p = S.profiles[appid];
    loadProfile(p ? p.data : S.base);
    fillProfiles(p ? appid : 'global');
    if (!quiet) toast(p ? `Perfil de «${p.name}» cargado.` : 'Perfil Global: valores por defecto del nivel.');
  }

  async function saveProfile() {
    const g = D.games[S.game];
    if (!g) { toast('Elige un juego de Steam para guardar su perfil.'); return; }
    const data = profileState();
    try {
      await api.saveProfile(g.appid, g.name, data);
      S.profiles[g.appid] = { name: g.name, data };
      fillProfiles(g.appid);
      toast(`Perfil de «${g.name}» guardado: se carga al elegir el juego${first('profile-auto-apply')?.checked ? ' y se aplica al iniciarlo' : ''}.`);
    } catch (err) {
      toast(`No se pudo guardar el perfil: ${err && err.message ? err.message : err}`);
      fillProfiles(g.appid);
    }
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
      if (b.dataset.v === 'normal') { setCheck('game-priority-persistent', false); toast('Prioridad Normal: se desmarca «Prioridad Alta siempre».'); }
    }
  }

  /* ---------- Sensores y FPS (eventos) ---------- */
  const fmtSensor = s => (s.value == null ? 'No disponible' : s.unit === 'c' ? Math.round(s.value) + ' °C' : s.unit === 'pct' ? Math.round(s.value) + ' %' : Math.round(s.value).toLocaleString('es') + ' rpm');

  /* Filas de ventiladores: se rehacen solo si cambia la lista de sensores. */
  function renderFans(fans) {
    const ids = fans.map(f => f.id).join('|');
    if (ids === S.fanIds) return;
    S.fanIds = ids;
    DS('sensor-fans').forEach(e => {
      e.innerHTML = '<div class="fan"><span>CPU</span><div class="b"><i data-meter="cpu"></i></div><span class="v" data-temp="cpu"></span></div>' +
        '<div class="fan"><span>GPU</span><div class="b"><i data-meter="gpu"></i></div><span class="v" data-temp="gpu"></span></div>' +
        fans.map((f, i) => `<div class="fan"><span title="${esc(f.label)}">${esc(f.label)}</span><div class="b" data-fanbar="${i}"></div><span class="v" data-fan="${i}"></span></div>`).join('');
    });
  }

  function onSensors(list) {
    S.lastSensors = list;
    const w = +(first('thermal-warn-threshold')?.value || 0), a = +(first('thermal-alert-threshold')?.value || 0);
    const pick = k => list.find(s => s.kind === k && s.value != null) || list.find(s => s.kind === k);
    const fans = list.filter(s => s.kind === 'fan');
    renderFans(fans);
    ['cpu', 'gpu'].forEach(k => {
      const s = pick(k), t = s && s.value != null ? Math.round(s.value) : null;
      const c = t == null ? null : t >= a ? 'bad' : t >= w ? 'warn' : 'ok';
      $$(`[data-temp="${k}"]`).forEach(e => {
        e.className = e.className.replace(/\s*(c-\w+|na)\b/g, '');
        if (t == null) { e.textContent = 'No disponible'; e.classList.add('na'); } else { e.textContent = t + ' °C'; e.classList.add('c-' + c); }
      });
      $$(`[data-meter="${k}"]`).forEach(m => { m.style.width = t == null ? '0' : Math.min(t, 100) + '%'; m.className = c ? 'bg-' + c : ''; });
      setText(`.${k}Src`, s ? s.source : 'sin sensor');
      if (c === 'bad' && first('thermal-alerts')?.checked && !S.alerted) { S.alerted = true; toast(`${k.toUpperCase()} a ${t} °C: supera la temperatura de alerta. Te recomendamos bajar al nivel Normal.`); }
    });
    fans.forEach((f, i) => {
      $$(`[data-fan="${i}"]`).forEach(e => {
        e.className = 'v' + (f.value == null ? ' na' : f.value === 0 ? ' c-bad' : '');
        e.innerHTML = f.value === 0 ? '0 rpm · parado' : esc(fmtSensor(f)) + (f.target ? ' <span class="na" title="El controlador da las RPM objetivo, no las reales">objetivo</span>' : '');
      });
      /* Solo hay escala si la fuente da %; las RPM no tienen máximo conocido. */
      $$(`[data-fanbar="${i}"]`).forEach(b => { b.innerHTML = f.unit === 'pct' && f.value != null ? `<i class="bg-ok" style="width:${Math.min(100, f.value)}%"></i>` : ''; });
    });
    $$('.fanCount').forEach(e => {
      const ok = fans.filter(x => x.value != null).length;
      e.innerHTML = fans.length ? `${ok}<span class="na"> / ${fans.length}</span>`
        : `<span class="na">No disponible${D.device.laptop ? ' · portátil' : ''}</span>`;
    });
    const src = [...new Set(list.map(s => s.source))];
    /* Sin ventiladores se dice por qué: en un portátil (PCSystemType = Mobile) ninguna fuente los expone. */
    const where = D.sensorSources.join(', ') || 'ninguna';
    const admin = D.sensorNeedsAdmin.length && !S.admin
      ? ` ${D.sensorNeedsAdmin.join(', ')} solo se puede leer con permisos de administrador: pulsa «${ELEVATE_LABEL[D.os].replace(/^\S+ /, '')}».` : '';
    const noFans = admin || (fans.length ? ''
      : D.device.laptop ? ` Es un portátil (según Windows): ni el sistema ni la GPU informan de sus ventiladores (consultado: ${where}).`
        : ` No se encontró ningún ventilador, tampoco en la GPU (consultado: ${where}).`);
    DS('sensor-backend-notice').forEach(e => {
      e.textContent = src.length ? `Sensores leídos de: ${src.join(', ')}.${pick('cpu') ? '' : ' Todavía no se puede leer la temperatura de la CPU en este equipo.'}${noFans}`
        : 'No se encontró ningún sensor de CPU, GPU ni ventiladores en este equipo.' + (D.device.laptop ? ' Es un portátil (según Windows).' : '') + admin;
    });
  }

  function onFps(v) {
    setText('.fpsNow', v == null ? '—' : String(v));
    const sp = $('#spk'); if (!sp) return;
    if (v == null) { S.fps = []; sp.setAttribute('points', ''); return; }
    S.fps.push(v); if (S.fps.length > 60) S.fps.shift();
    const W = +sp.dataset.w, H = +sp.dataset.h, lo = Math.min(...S.fps), hi = Math.max(...S.fps), span = hi - lo || 1;
    sp.setAttribute('points', S.fps.map((x, i) => `${(i * W / 59).toFixed(1)},${(H - (x - lo) / span * H).toFixed(1)}`).join(' '));
  }

  /* ---------- Eventos de la UI ---------- */
  document.addEventListener('click', e => {
    const t = e.target;
    const rel = t.closest('[data-release]'); if (rel) { e.preventDefault(); releaseRow(rel.dataset.release); return; }
    const lim = t.closest('[data-limit]'); if (lim) { e.preventDefault(); limitRow(lim.dataset.limit); return; }
    const lb = t.closest('[data-l]'); if (lb) { setLevel(+lb.dataset.l); return; }
    const bk = t.closest('[data-setting="process-category-bulk"]');
    if (bk) { e.preventDefault(); D.processes.forEach((p, i) => { if (p.category === bk.dataset.c && !locked(p)) S.chk[i] = bk.dataset.v === '1'; }); render(); return; }
    const sg = t.closest('.win .seg>button');
    if (sg && !sg.disabled) { const g = sg.parentElement; [...g.children].forEach(x => x.classList.toggle('on', x === sg)); onSeg(g.dataset.setting, sg); }
    if (t.closest('[data-cwok]')) {
      if (!$('#cwAck').checked) { toast('Marca «Entiendo lo que deja de funcionar».'); return; }
      DS('compat-warning').forEach(x => { x.hidden = true; }); $('#cwAck').checked = false;
      const cw = S.cwRetry || { verb: 'Aplicar', turbo: false }; S.cwRetry = null;
      apply(true, cw.verb, cw.turbo); return;
    }
    if (t.closest('[data-cw]')) { S.cwRetry = null; DS('compat-warning').forEach(x => { x.hidden = true; }); return; }
    if (t.closest('[data-rb]')) { DS('reboot-required').forEach(x => { x.hidden = true; }); return; }
    const cr = t.closest('[data-crash]');
    if (cr) {
      DS('restore-after-crash').forEach(x => { x.hidden = true; });
      if (cr.dataset.crash === 'restore') revert(); else toast('Aviso descartado. Los cambios siguen anotados: puedes deshacerlos cuando quieras con «Restaurar».');
      return;
    }
    const wb = t.closest('[data-win]');
    if (wb) { if (!api.inTauri) toast('Minimizar y cerrar solo funcionan dentro de la app.'); else api.win(wb.dataset.win); return; } /* La bandeja llega con el overlay (M6) */

    const el = t.closest('button[data-setting]'); if (!el) return;
    const id = el.dataset.setting;
    if (id === 'apply') apply();
    if (id === 'undo-last') undo();
    if (id === 'restore') revert();
    if (id === 'launch-game') launchGame();
    if (id === 'status-view') { const v = $('#stv'); v.hidden = !v.hidden; if (!v.hidden) v.textContent = statusText(); }
    if (id === 'realtime-confirm') {
      if (!$('#rtAck').checked) { toast('Marca «Entiendo el riesgo» para confirmar.'); return; }
      S.rt = true; $$('[data-setting="game-priority"] button').forEach(x => x.classList.toggle('on', x.dataset.v === 'realtime'));
      $('#rtBox').hidden = true; $('#rtAck').checked = false; toast('Prioridad Tiempo real confirmada: se usará al aplicar.');
    }
    if (id === 'run-as-admin') elevate();
    if (id === 'profile-save') saveProfile();
    if (id === 'launch-options-copy') copyLaunchOptions();
    if (id === 'export-diagnostics') toast(`Exportar el diagnóstico ${NOT_WIRED}`);
    if (id === 'always-on-top') { const on = el.getAttribute('aria-pressed') !== 'true'; el.setAttribute('aria-pressed', on); el.classList.toggle('on', on); api.win('always-on-top', on); }
  });

  document.addEventListener('input', e => {
    if (['keep-alive-list', 'thermal-alert-threshold', 'thermal-warn-threshold'].includes(e.target.dataset.setting)) e.target.dataset.touched = 1;
  });

  document.addEventListener('change', e => {
    const t = e.target, id = t.dataset.setting;
    if (t.dataset.i != null) { S.chk[+t.dataset.i] = t.checked; t.closest('.prow')?.classList.toggle('off', !t.checked); summary(); return; }
    if (t.dataset.s != null) { S.stc[+t.dataset.s] = t.checked; render(); return; }
    if (id === 'game-target') {
      S.game = +t.value;
      const g = game();
      /* Sin perfil propio, un perfil de otro juego no se queda puesto: vuelve a Global. */
      if (g.appid in S.profiles || first('profile')?.value !== 'global') useProfile(g.appid, true); else setLevel(S.level);
      toast(`Juego: ${g.name}${g.appid in S.profiles ? ' · perfil cargado' : ''}`);
    }
    if (id === 'cpu-affinity') syncAffinityCores();
    if (id === 'custom-power-plan') renderPower();
    if (id === 'game-priority-persistent' && t.checked && priorityValue() === 'normal') { t.checked = false; toast('«Prioridad Alta siempre» solo guarda la prioridad Alta: elige Alta primero.'); }
    if (id === 'vbs-hvci-off' && t.checked) toast('Desactivar VBS/HVCI: al aplicar tendrás que confirmar qué deja de funcionar.');
    if (id === 'fps-cap' && +t.value > 0) toast(`Límite de FPS ${t.value}: al aplicar se guarda como fps_max en framerush.cfg.`);
    if (id === 'resolution-override' && t.value === 'custom') toast('Resolución personalizada: control pendiente de definir.');
    if (id === 'profile') {
      if (t.value === 'nuevo') saveProfile();
      else if (t.value === 'global') useProfile(null);
      else {
        /* El perfil va con su juego: si está instalado, se elige también en game-target. */
        const i = D.games.findIndex(g => String(g.appid) === t.value);
        if (i >= 0) { S.game = i; DS('game-target').forEach(s => { s.value = String(i); }); caps(); }
        useProfile(+t.value);
      }
    }
    if (id === 'language') toast(t.value === 'en' ? 'Language: English (vista previa; i18n pendiente)' : 'Idioma: español');
    if (id === 'dry-run') setText('[data-setting="apply"] .lbl', t.checked ? 'Simular optimización' : 'Aplicar optimización');
    if (id === 'auto-revert-on-exit') sess();
    if (id === 'thermal-alerts' || id === 'thermal-alert-threshold') S.alerted = false;
  });

  document.addEventListener('keydown', e => { if (e.ctrlKey && e.key === 'Enter') { e.preventDefault(); apply(); } }); /* shortcut-apply */

  /* ---------- Arranque ---------- */
  /* De momento el helper solo lee los sensores que piden admin; aplicar sigue sin conectar. */
  async function elevate() {
    DS('run-as-admin').forEach(b => { b.disabled = true; });
    try {
      await api.elevate();
      toast('Permisos de administrador activos: ya se leen los sensores que los necesitan.');
      await init();
    } catch (err) {
      toast(`No se pudieron obtener permisos de administrador: ${err && err.message ? err.message : err}`);
      elevateButton();
    }
  }

  function elevateButton() {
    DS('run-as-admin').forEach(b => { b.disabled = S.admin; b.textContent = S.admin ? '✓ Permisos de administrador activos' : ELEVATE_LABEL[D.os]; });
  }

  async function init() {
    stopStreams.forEach(f => f()); stopStreams = [];
    try {
      D = await api.load();
    } catch (err) {
      $('.scroll').innerHTML = `<p class="note" role="alert">${esc(err && err.message ? err.message : err)}</p>`;
      return;
    }
    /* Juegos de Steam del equipo; adaptadores y recomendaciones llegan en M4. */
    D.steamRecs = D.steamRecs || [];
    Object.assign(S, { admin: D.elevated, game: Math.max(0, D.games.findIndex(g => g.running)), hist: [], rt: false, alerted: false, rb: [], turbo: false, fps: [], lastSensors: null, fanIds: '', released: [], chk: [], stc: D.steamRecs.map(s => !!s.checked) });
    $$('[data-touched]').forEach(x => { delete x.dataset.touched; });

    DS('game-target').forEach(s => {
      s.innerHTML = D.games.length
        ? D.games.map((g, i) => `<option value="${i}">${esc(g.name)}${g.running ? ' (en ejecución)' : ''}${g.antiCheat ? ' · ' + esc(g.antiCheat.name) : ''}</option>`).join('')
        : `<option value="0">${esc(NO_GAME.name)}</option>`;
      s.value = String(S.game);
      s.disabled = !D.games.length;
    });
    DS('launch-game').forEach(b => { b.disabled = !D.games.length; b.title = D.games.length ? '' : D.gamesNote || ''; });
    $('#steamList').closest('details').hidden = !D.steamRecs.length;
    DS('restore-after-crash').forEach(e => { e.hidden = !D.pendingSession; });
    setText('.crashN', D.pendingSession ? D.pendingSession.changes : '');
    setText('.rtApi', RT_API[D.os] || '');
    DS('compat-warning').forEach(e => { e.hidden = true; }); rebootBanner();
    elevateButton();
    setLevel(DEFAULT_LEVEL); sess(); renderPerf();
    /* Global = lo de arriba; el perfil del juego en marcha, si lo hay, se carga encima. */
    S.base = profileState();
    try { S.profiles = await api.profiles(); } catch (err) { S.profiles = {}; toast(`No se pudieron leer los perfiles: ${err && err.message ? err.message : err}`); }
    const g = D.games[S.game];
    if (g && g.appid in S.profiles) useProfile(g.appid); else fillProfiles('global');
    stopStreams.push(api.onSensors(onSensors), api.onFps(onFps), api.onGameExited(onGameExited), api.onGameLaunched(onGameLaunched), api.onTurboRescan(onTurboRescan));
  }

  DS('hardware-profile').forEach(e => { e.textContent = 'Leyendo el equipo…'; });
  init();
})();

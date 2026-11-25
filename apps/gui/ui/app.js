/*
 * FrameRush GUI (diseño final: design/v3-overlay-compact.html).
 * Ids de data-setting, valores y defaults: design/SETTINGS.md. Reglas de comportamiento: SETTINGS.md §4 y §6.
 * Todo lo que depende del equipo llega en el snapshot de FrameRushApi.load() (R8); aquí solo hay
 * textos de la interfaz y reglas del plan. Lo que el snapshot no trae se oculta o se muestra
 * como "No disponible": nunca se rellena.
 */
(async function () {
  'use strict';

  const api = window.FrameRushApi;
  const I18N = window.FrameRushI18n;
  /* Sin los textos la app sigue: t() devuelve el id (se ve qué falta, no se inventa). */
  try { await I18N.ready; } catch (err) { console.error('i18n:', err); }
  const t = I18N.t;

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
  const CAT = Object.fromEntries(['telemetry', 'updater', 'sync', 'launcher', 'browser', 'overlay', 'peripheral', 'service_a', 'service_b'].map(c => [c, t(`cat-${c}`)]));
  const ACT = { lower_priority: t('act-lower-priority'), suspend: t('act-suspend'), kill: t('act-kill'), stop_service: t('act-stop-service') };
  const RISK = { low: [t('risk-low'), 1], medium: [t('risk-medium'), 2], high: [t('risk-high'), 3] };
  const LV = { normal: 1, agresivo: 2, extremo: 3 };
  const LV_NAME = ['', t('level-normal'), t('level-agresivo'), t('level-extremo')];
  const LV_NOTE = ['', t('level-note-normal'), t('level-note-agresivo'), t('level-note-extremo')];
  const DEFAULT_LEVEL = 2; /* Agresivo (PLAN §3, valores por defecto) */
  /* Ajustes que requieren reinicio (SETTINGS §4.7). */
  const REBOOT = { hags: 'HAGS', 'vbs-hvci-off': t('reboot-vbs-off'), 'timer-resolution': t('reboot-timer') };
  /* run-as-admin: W y L según SETTINGS.md; la etiqueta de macOS no está definida (pendiente). */
  const ELEVATE_LABEL = { win: t('elevate-win'), lin: t('elevate-lin'), mac: t('elevate-mac') };
  const RT_API = { lin: ' (SCHED_RR)' };
  const FPS_SOURCE = { presentmon: 'PresentMon (ETW)', mangohud: t('fps-source-mangohud') };
  const GPU_NOTE = { write: t('gpu-note-write'), info: t('gpu-note-info') };
  const POWER_BACKEND = { win: t('power-backend-win'), lin: 'power-profiles-daemon', mac: 'pmset' };
  /* Motivo de cada protegido (core::ProtectedReason). */
  const PROT_WHY = { os_critical: t('prot-os-critical'), compositor: t('prot-compositor'), audio: t('prot-audio'), gpu_driver: t('prot-gpu-driver'), anti_cheat: t('prot-anti-cheat'), self_app: 'FrameRush' };
  /* Ajustes con backend: se ocultan si el snapshot no los trae en `available` (P15). */
  const BACKEND_IDS = ['game-dvr-off', 'windows-game-mode', 'notifications-off', 'visual-effects-performance', 'uwp-background-off',
    'scheduler-foreground-boost', 'power-throttling-off', 'mmcss-games', 'defender-exclusions', 'hags', 'vbs-hvci-off', 'cstates-off',
    'memory-trim', 'standby-purge', 'custom-power-plan', 'refresh-rate-max', 'game-priority-persistent', 'fullscreen-optimizations-off',
    'gpu-preference', 'cpu-affinity', 'timer-resolution'];
  const NO_GAME = { name: t('no-game'), exe: '', appid: '', pids: [] };
  const GB = 1024 ** 3, MB = 1024 ** 2;

  /* ---------- Estado ---------- */
  let D = null;           /* snapshot del backend */
  let stopStreams = [];
  const S = { level: DEFAULT_LEVEL, admin: false, game: 0, plan: null, hist: [], watching: false, profiles: {}, base: null, rt: false, alerts: {}, rb: [], turbo: false, fps: [], chk: [], stc: [], fanIds: '', protOpen: false, released: [], turboPending: null };

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

  function toast(msg, ms = 3200) {
    let t = $('#toast');
    if (!t) { t = document.createElement('div'); t.id = 'toast'; t.className = 'toast'; t.setAttribute('role', 'status'); document.body.appendChild(t); }
    t.textContent = msg; t.classList.add('show');
    clearTimeout(t._h); t._h = setTimeout(() => t.classList.remove('show'), ms);
  }
  const setCheck = (id, v) => DS(id).forEach(x => { x.checked = v; });

  /*
   * Estados de espera (design/estados-espera.html, variante C): tarjeta con anillo sobre la
   * ventana mientras se carga, aplica, deshace, restaura o Windows pide el UAC. El progreso es el
   * real del backend (evento `progress`): fases de la carga y cada paso terminado del motor.
   * Sale tras WAIT_DELAY ms para que lo instantáneo no parpadee; el UAC sale enseguida.
   */
  const WAIT_DELAY = 200;
  const LOAD_PHASES = ['device', 'rules', 'processes', 'steam'];
  const RING = 238.76; /* 2·π·38, el radio del anillo */
  const W = { op: null, timer: null, done: 0, total: null, what: '' };
  function waitShow(op) {
    clearTimeout(W.timer);
    Object.assign(W, { op, done: 0, total: op === 'load' ? LOAD_PHASES.length : null, what: '' });
    const open = () => { if (W.op === op) { $('#wait').hidden = false; waitRender(); } };
    if (op === 'uac') open(); else W.timer = setTimeout(open, WAIT_DELAY);
  }
  function waitHide() {
    clearTimeout(W.timer); W.op = null;
    $('#wait').hidden = true;
  }
  function onProgress(e) {
    if (e.op !== W.op) return; /* p. ej. «Actualizar lista» también carga, sin tarjeta */
    Object.assign(W, { done: e.done, total: e.total ?? null, what: e.what });
    if (!$('#wait').hidden) waitRender();
  }
  function waitRender() {
    const box = $('#wait'), ring = box.querySelector('.ring'), fg = box.querySelector('.fg');
    const uac = W.op === 'uac', load = W.op === 'load';
    box.querySelector('.waitBrand').hidden = !load;
    box.querySelector('.waitTitle').textContent = t('wait-' + W.op);
    box.querySelector('.waitUac').hidden = !uac;
    ring.classList.toggle('pause', uac);
    /* Sin total no hay fracción honesta: el anillo gira y el centro cuenta los pasos hechos. */
    ring.classList.toggle('spin', !uac && W.total == null);
    fg.style.strokeDashoffset = !uac && W.total != null ? String(RING * (1 - Math.min(W.done, W.total) / W.total)) : '';
    const num = box.querySelector('.num');
    if (uac) num.innerHTML = '<span class="big" aria-hidden="true">🛡</span>';
    else if (W.total != null) num.innerHTML = `${Math.min(W.done, W.total)}<small>${esc(t('wait-of', { total: W.total }))}</small>`;
    else num.innerHTML = `${W.done}<small>${esc(t('wait-steps', { n: W.done }))}</small>`;
    /* Carga: la fase en curso es la siguiente a la última terminada. Resto: el último paso hecho. */
    let now = '';
    if (load) now = t('wait-phase-' + LOAD_PHASES[Math.min(W.done, LOAD_PHASES.length - 1)]);
    else if (!uac) now = W.what || t('wait-starting');
    box.querySelector('.waitNow').textContent = now;
    const dots = W.total != null && W.total <= 12 && !uac
      ? Array.from({ length: W.total }, (_, i) => `<i class="${i < W.done ? 'd' : i === W.done ? 'c' : ''}"></i>`).join('') : '';
    box.querySelector('.waitDots').innerHTML = dots;
  }

  /* ---------- Plantillas ---------- */
  const actBadge = p => {
    const a = effAct(p), fell = a !== p.action;
    return `<span class="act" data-setting="process-action" title="${fell ? t('act-fell-title') : ACT[a]}">${ACT[a]}${fell ? ' *' : ''}</span>`;
  };
  const lockBadge = p => (locked(p) ? ` <span class="adm" data-setting="admin-lock">${t('admin-lock')}</span>` : '');
  const riskBadge = p => { const r = RISK[p.risk]; return `<span class="badge r${r[1]}">${r[0]}</span>`; };
  /* Reanudar / volver a limitar una fila sin tocar el resto de la sesión. */
  const rowBtn = p => (p.limited
    ? ` <button class="btn rowAct" data-release="${esc(p.id)}" data-setting="process-release" title="${t('row-release-title')}">${t('row-release')}</button>`
    : S.released.includes(p.id) ? ` <button class="btn rowAct" data-limit="${esc(p.id)}" data-setting="process-release" title="${t('row-limit-title')}">${t('row-limit')}</button>` : '');
  const rowHTML = (p, i) => `<label class="p prow ${S.chk[i] ? '' : 'off'} ${locked(p) ? 'lk' : ''}" title="${esc(p.desc)}"><input type="checkbox" data-i="${i}" ${S.chk[i] ? 'checked' : ''} ${locked(p) ? 'disabled' : ''}><span class="n"><b>${esc(p.name)}</b>${lockBadge(p)}${p.limited ? ` <span class="badge r2">${t('row-limited')}</span>` : ''}<i>${esc(p.desc)}</i></span><span class="r">${actBadge(p)} ${p.ramBytes ? fmt(p.ramBytes) : '—'} · ${p.cpu == null ? '— CPU' : p.cpu.toFixed(1) + '%'}${riskBadge(p)}${rowBtn(p)}</span></label>`;
  const catHTML = c => `<div class="cat">${CAT[c]}<span class="sp"></span><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="1">${t('bulk-all')}</button><button class="lnk" data-setting="process-category-bulk" data-c="${c}" data-v="0">${t('bulk-none')}</button></div>`;
  const lockHTML = l => `<div class="p lk" data-setting="protected-processes"><span>🔒</span><span class="n"><b>${esc(l.name)}</b><i>${esc(l.why)} · ${t('prot-never')}</i></span><span class="r"><span class="badge r0">${t('prot-badge')}</span></span></div>`;
  const stHTML = (s, i, dis) => `<label class="p ${dis ? 'dis' : ''}"><input type="checkbox" data-s="${i}" ${S.stc[i] ? 'checked' : ''} ${dis ? 'disabled' : ''}><span class="n"><b>${esc(s.name)}</b><span class="src">${esc(s.source)}</span><i>${esc(s.why)}${dis ? ' · ' + t('steam-needs-overlay-off') : ''}</i></span><span></span></label>`;

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
      if (s && s.dataset.o) s.textContent = ok ? s.dataset.o : t('available-from', { level: LV_NAME[LV[r.dataset.min]] });
    });
    const ac = antiCheat(), vb = D.vbsBlocked;
    if (ac) setCheck('vbs-hvci-off', false); /* P10: desmarcado con cualquier anticheat */
    DS('vbs-hvci-off').forEach(x => {
      if (vb) { x.disabled = true; x.checked = false; } /* P10, V7: bloqueado con Vanguard/FACEIT (core) */
      const n = x.closest('.row')?.querySelector('.vbsAc');
      if (n) { n.hidden = !vb; n.textContent = vb ? t('vbs-blocked', { reason: vb }) : ''; }
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
    if (!D.processes.length) h += `<p class="note">${t('no-processes')}</p>`;
    /* Solo informativos: plegados por defecto; se recuerda si el usuario los abre (render rehace la lista). */
    const prot = $('#rows details.prot');
    if (prot) S.protOpen = prot.open;
    if (D.protected.length) {
      h += `<details class="adv prot" ${S.protOpen ? 'open' : ''}><summary>${t('prot-summary', { n: D.protected.length })}</summary>`;
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
    DS('impact-estimate').forEach(e => { e.innerHTML = t('impact', { ram: `<b>${fmt(ram)}</b>`, cpu: `<b>${cpu.toFixed(1)} %</b>`, n }); });
    setText('.pm', t('checked-of', { n, total: D.processes.length }));
    setText('.stm', t('steam-recs-of', { n: S.stc.filter(Boolean).length, total: D.steamRecs.length }));
  }

  /* ---------- Energía (P3) ---------- */
  function renderPower() {
    const o = D.device.power;
    if (!o) {
      DS('power-plan').forEach(g => { g.innerHTML = ''; });
      setText('.pwrCur', t('unavailable')); setText('.pwrNote', t('power-unreadable'));
      return;
    }
    DS('power-plan').forEach(g => {
      g.innerHTML = o.plans.map((x, i) => `<button data-i="${i}" class="${i === S.plan ? 'on' : ''}" title="${i === o.current ? t('power-current-title') : ''}">${esc(x)}${i === o.current ? ' •' : ''}</button>`).join('');
    });
    setText('.pwrCur', o.current == null ? t('power-current-unknown') : o.plans[o.current]);
    const custom = D.caps.windows && active('custom-power-plan');
    /* cstates-off va dentro de «FrameRush Max»: sin ese plan no hay dónde ponerlo. */
    DS('cstates-off').forEach(x => { x.disabled = !custom; });
    $$('.cstNote').forEach(e => { e.hidden = custom; });
    const what = custom ? t('power-will-custom', { base: o.plans[S.plan] ?? t('power-current-plan') })
      : S.plan == null || S.plan === o.current ? t('power-no-change') : t('power-will-switch', { plan: o.plans[S.plan] });
    setText('.pwrNote', t('power-note', { backend: POWER_BACKEND[D.os], what }));
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
      e.textContent = ac ? t('anticheat-warning', { name: antiCheat().name }) : '';
    });
    setText('.fpsNote', k.fps ? FPS_SOURCE[k.fps] : t('fps-none-yet'));
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
      ? (cpu.cores.some(c => c.efficiency) ? `${cpu.cores.filter(c => !c.efficiency).length}P + ${cpu.cores.filter(c => c.efficiency).length}E` : t('physical-cores', { n: cpu.cores.length })) + ' / ' + t('threads', { n: cpu.threads })
      : t('threads', { n: cpu.threads });
    const gpus = d.gpus.map(x => {
      const n = esc(x.name) + (x.vramBytes ? ` ${gb(x.vramBytes)}` : '');
      return x.preferred ? `<b>${n}</b>` : n;
    }).join(' + ');
    DS('hardware-profile').forEach(e => {
      e.innerHTML = [cpu ? `<b>${esc(cpu.model)}</b> · ${cores}` : `CPU: ${t('unavailable')}`, gpus || `GPU: ${t('unavailable')}`, d.manufacturer ? esc(d.manufacturer) : '']
        .filter(Boolean).join(' · ');
    });

    /* gpu-preference: la GPU que elige core (más VRAM dedicada) o dejarlo al sistema */
    DS('gpu-preference').forEach(sel => {
      sel.innerHTML = d.gpus.filter(x => x.preferred).map(x => `<option value="alto">${esc(x.name)}${x.vramBytes ? ` ${gb(x.vramBytes)}` : ''}</option>`).join('') +
        `<option value="sistema">${t('gpu-system')}</option>`;
    });

    /* resolution-override: modos reales del monitor principal (R8); nativa = no se toca */
    DS('resolution-override').forEach(sel => {
      const m = d.displays.find(x => x.primary) || d.displays[0];
      sel.innerHTML = (m ? `<option value="nativa">${t('res-native-size', { w: m.width, h: m.height })}</option>` : `<option value="nativa">${t('res-native')}</option>`) +
        (m ? m.resolutions.filter(([w, h]) => w !== m.width || h !== m.height)
          .map(([w, h]) => `<option value="${w}x${h}">${w}×${h}${w < 1.4 * h ? ' · 4:3' : ''}</option>`).join('') : '');
    });

    const ka = first('keep-alive-list');
    if (ka && !ka.dataset.touched) ka.value = d.keepAlive.value.join(', ');

    /* Umbrales térmicos: core::defaults (min(TjMax − 10, slowdown GPU − 5); aviso = alerta − 15) */
    const al = first('thermal-alert-threshold'), wa = first('thermal-warn-threshold');
    if (al && !al.dataset.touched) al.value = Math.round(d.thermalAlert.value);
    if (wa && !wa.dataset.touched) wa.value = Math.round(d.thermalWarn.value);
    const slowdown = d.gpus.find(x => x.slowdown != null)?.slowdown ?? null;
    setText('.thrNote', d.thermalAlert.estimated ? t('thr-estimated')
      : t('thr-calculated', { tjmax: cpu?.tjmax ?? '—', slowdown: slowdown ?? '—', alert: Math.round(d.thermalAlert.value) }));

    /* manual-recommendations (It5.8): rules/recommendations.toml filtradas por core para este equipo y el juego elegido */
    const recs = [...D.recommendations, ...(game().recommendations || [])];
    DS('manual-recommendations').forEach(e => {
      e.innerHTML = recs.map(r => `<label><input type="checkbox"><span><b>${esc(r.title)}</b>${r.steps.map(s => `<br>${esc(s)}`).join('')}<br><small>${t('rec-source', { source: esc(r.source) })}</small></span></label>`).join('');
      e.closest('details').hidden = !recs.length;
    });

    /* compat-warning: solo lo que está instalado */
    $$('.cwList').forEach(e => {
      e.innerHTML = (d.virtualization.length ? d.virtualization.map(v => `<li>${t('cw-installed', { name: esc(v) })}</li>`).join('') : `<li>${t('cw-none')}</li>`) +
        (D.vbsBlocked ? `<li>${t('cw-blocked', { reason: esc(D.vbsBlocked) })}</li>` : ac ? `<li>${t('cw-anticheat', { name: esc(ac.name) })}</li>` : '');
    });

    setText('.affNote', d.affinityWarning ? t('aff-warning', { n: cpu.cores.length }) : t('aff-optional'));
    renderAffinity();
    if (S.lastSensors) onSensors(S.lastSensors);
  }

  /* cpu-affinity-cores: la topología real; marcadas las que propone core::defaults::affinity_cpus */
  function renderAffinity() {
    const b = first('cpu-affinity-cores'); if (!b) return;
    const cpu = D.device.cpu, on = new Set(D.device.affinity.value);
    const cpus = cpu ? cpu.cores.flatMap((c, k) => c.logical.map(i => ({ i, k, e: c.efficiency }))) : [];
    b.innerHTML = cpus.sort((x, y) => x.i - y.i).map(({ i, k, e }) =>
      `<input type="checkbox" id="core${i}" ${on.has(i) ? 'checked' : ''}><label for="core${i}" title="${t('core-title', { core: k, cpu: i })}">${e ? 'E' : 'P'}${i}</label>`).join('');
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
    setText('.perfSum', t('perf-none'));
  }

  /* ---------- Sesión ---------- */
  function rebootBanner() {
    DS('reboot-required').forEach(e => { e.hidden = !S.rb.length; const l = e.querySelector('.rbList'); if (l) l.textContent = S.rb.map(x => REBOOT[x]).join(', '); });
  }

  /* Lo seleccionado en la UI; lo aplicado de verdad lo da `framerush status` hasta que la app aplique. */
  function statusText() {
    const pw = D.device.power, g = game();
    const rows = [
      [t('st-level'), LV_NAME[S.level]],
      [t('st-processes'), t('st-checked', { n: S.chk.filter(Boolean).length })],
      [t('st-power'), (pw && S.plan != null ? pw.plans[S.plan] : t('st-no-change')) + (active('custom-power-plan') ? ' + FrameRush Max' : '')],
      [t('st-priority'), (S.rt ? t('prio-realtime') : priorityValue() === 'normal' ? t('prio-normal') : t('prio-high')) + (active('game-priority-persistent') ? ' ' + t('st-always') : '')],
      [t('st-refresh'), active('refresh-rate-max') ? t('st-max') : t('st-no-change')],
      [t('st-game'), g.name],
      [t('st-keep-alive'), first('keep-alive-list')?.value || '—'],
      [t('st-previous'), D.pendingSession ? t('st-unrestored', { n: D.pendingSession.changes }) : t('st-no')],
    ];
    const pad = Math.max(...rows.map(r => r[0].length));
    return rows.map(([k, v]) => `${k.padEnd(pad)} : ${v}`).join('\n');
  }

  /* Vista Estado: lo elegido, lo aplicado (journal) y el equipo ahora, leído por el backend. */
  async function showStatus(v) {
    v.textContent = statusText() + '\n\n' + t('st-reading');
    try {
      const st = await window.FrameRushApi.status();
      const scope = { session: t('scope-session'), persistent: t('scope-persistent') };
      const state = { done: t('state-done'), pending: t('state-pending'), failed: t('state-failed') };
      const applied = st.session.length
        ? st.session.map(l => '  ' + t('st-batch', { batch: l.batch, scope: scope[l.scope] || l.scope, state: state[l.status] || l.status, what: l.what }))
        : ['  ' + t('st-nothing')];
      const pad = Math.max(...st.system.map(l => l.label.length), 0);
      v.textContent = [statusText(), '', t('st-applied'), ...applied, '', t('st-system'),
        ...st.system.map(l => `  ${l.label.padEnd(pad)}  ${l.value}`)].join('\n');
    } catch (err) {
      v.textContent = statusText() + '\n\n' + t('st-error', { error: String(err && err.message ? err.message : err) });
    }
  }

  function sess() {
    const on = S.hist.length > 0;
    DS('session-status').forEach(e => {
      e.classList.toggle('on', on);
      const t = e.querySelector('.sessT');
      if (t) t.textContent = on ? I18N.t('session-on', { changes: S.hist.reduce((a, b) => a + b, 0), applies: S.hist.length }) + (S.watching ? ' · ' + I18N.t('session-watching') : '') : I18N.t('session-off');
    });
    DS('restore').forEach(b => { b.disabled = !on && !D.pendingSession; });
    DS('undo-last').forEach(b => { b.disabled = !on; });
  }

  const levelToggles = () => $$('.tg[data-setting]').filter(x => x.checked && !x.disabled && shown(x) && x.closest('[data-min]'));

  /* Informe de apply/undo/revert: hechos, fallidos y lo que no se pudo (sin inventar nada). */
  function report(verb, r) {
    const fails = r.lines.filter(l => l.outcome === 'failed');
    let msg = t('rep-done', { verb, done: r.done }) + (r.failed ? ' · ' + t('rep-failed', { n: r.failed }) : '') + '.';
    if (fails.length) msg += ' ' + t('rep-first-fail', { what: fails[0].what, detail: fails[0].detail });
    if (r.needsAdmin.length) msg += ' ' + t('rep-needs-admin', { button: ELEVATE_LABEL[D.os].replace(/^\S+ /, ''), list: r.needsAdmin.join(', ') });
    if (r.notWired.length) msg += ' ' + t('rep-not-wired', { list: r.notWired.join(', ') });
    if (r.notes.length) msg += ' ' + t('rep-notes', { list: r.notes.join('; ') });
    if (!r.lines.length && !r.notWired.length && !r.needsAdmin.length && !r.notes.length) msg = t('rep-nothing', { verb });
    /* reboot-required: los ajustes de REBOOT que se aplicaron (SETTINGS §4.7). El texto del informe
       lleva el id del ajuste en cualquier idioma. */
    r.lines.filter(l => l.outcome === 'done')
      .forEach(l => Object.keys(REBOOT).filter(id => l.what.includes(id) && !S.rb.includes(id)).forEach(id => S.rb.push(id)));
    rebootBanner();
    toast(msg);
  }

  /* `wait`: tarjeta de espera ('apply', 'undo', 'revert') o nada. */
  async function run(verb, op, wait = null) {
    DS('apply').forEach(b => { b.disabled = true; });
    if (wait) waitShow(wait);
    try {
      const r = await op();
      report(verb, r);
      return r;
    } catch (err) {
      toast(`${verb}: ${err && err.message ? err.message : err}`);
      return null;
    } finally {
      if (wait) waitHide();
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
    } catch (err) { toast(t('err-limited', { error: String(err && err.message ? err.message : err) })); }
  }

  /*
   * foreign-frozen: procesos congelados por otro programa (p. ej. un script «Turbo» externo que
   * se cerró sin reanudarlos). Se mira al abrir y con ↻ Actualizar lista.
   */
  async function checkFrozen() {
    let list = [];
    try { list = await api.frozenProcesses(); } catch (err) { toast(t('err-frozen', { error: String(err && err.message ? err.message : err) })); }
    DS('foreign-frozen').forEach(box => {
      box.hidden = !list.length;
      const rows = box.querySelector('.frzList');
      if (rows) rows.innerHTML = list.map(p => `<div class="frzRow"><b>${esc(p.name)}</b><span>pid ${p.pid} · ${fmt(p.memoryBytes)}${p.admin ? ' · ' + esc(t('frz-admin')) : ''}</span><button class="btn sm" data-frz="${p.pid}">${esc(t('frz-resume'))}</button></div>`).join('');
    });
  }

  async function resumeForeign(pid) {
    const r = await run(t('verb-resume-foreign'), () => api.resumeForeign(pid === 'all' ? [] : [Number(pid)]));
    if (r) checkFrozen();
  }

  /*
   * process-refresh: vuelve a leer qué hay abierto sin tocar nivel, ajustes, perfil ni sesión.
   * Las filas que siguen conservan lo marcado; las nuevas llegan con lo que marca el nivel (core).
   */
  async function refreshList(btn) {
    btn.disabled = true;
    try {
      const fresh = await api.load();
      const was = new Map(D.processes.map((p, i) => [p.id, !!S.chk[i]]));
      const added = fresh.processes.filter(p => !was.has(p.id)).length;
      const gone = D.processes.filter(p => !fresh.processes.some(q => q.id === p.id)).length;
      D.processes = fresh.processes;
      D.protected = fresh.protected;
      S.chk = D.processes.map(p => !locked(p) && (was.has(p.id) ? was.get(p.id) : p.checked[S.level - 1]));
      S.released = S.released.filter(id => D.processes.some(p => p.id === id));
      render();
      checkFrozen();
      toast(added || gone ? t('list-updated', { added, gone }) : t('list-unchanged'));
    } catch (err) {
      toast(t('err-list', { error: String(err && err.message ? err.message : err) }));
    } finally {
      btn.disabled = false;
    }
  }

  async function releaseRow(id) {
    const r = await run(t('verb-release'), () => api.releaseRow(id));
    if (!r) return;
    if (!r.failed) S.released.push(id);
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; }
    sess(); syncLimited();
  }

  async function limitRow(id) {
    const r = await run(t('verb-limit'), () => api.limitRow(id, S.level, keepAliveList()));
    if (!r) return;
    if (r.done) S.hist.push(r.done);
    sess(); syncLimited();
  }

  async function undo() {
    const r = await run(t('verb-undo'), api.undo, 'undo');
    if (!r) return;
    S.hist.pop();
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; }
    sess(); syncLimited();
  }

  async function revert() {
    const r = await run(t('verb-restore'), api.revert, 'revert');
    if (!r) return;
    S.watching = false;
    if (!r.sessionOpen) { S.hist = []; D.pendingSession = null; DS('restore-after-crash').forEach(x => { x.hidden = true; }); }
    S.released = [];
    sess(); syncLimited();
  }

  /* El backend vuelve a planificar con el equipo de ahora y aplica solo lo marcado. */
  async function apply(force, verb = t('verb-apply'), turbo = false) {
    const n = S.chk.filter(Boolean).length + S.stc.filter(Boolean).length + levelToggles().length;
    if (first('dry-run')?.checked) { toast(t('dry-apply', { n })); return; }
    if (!force && active('vbs-hvci-off')) { /* SETTINGS §4.8 */
      /* La confirmación vuelve a aplicar igual: con Turbo si esto era Turbo. */
      S.cwRetry = { verb, turbo };
      DS('compat-warning').forEach(e => { e.hidden = false; });
      toast(t('cw-read-first'));
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
    }), 'apply');
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
        resolution: resolutionValue(),
      } : null,
    };
  }

  /* resolution-override: [ancho, alto] o null con «Nativa». */
  function resolutionValue() {
    const x = first('resolution-override');
    const m = x && !x.disabled && shown(x) ? /^(\d+)x(\d+)$/.exec(x.value) : null;
    return m ? [+m[1], +m[2]] : null;
  }

  /* Watchdog (It1.4, P5): el juego se cerró y el backend revirtió lo de sesión. */
  function onGameExited(e) {
    S.watching = false;
    if (D.games[S.game]) D.games[S.game].running = false;
    const resumedOnly = S.turbo && !first('auto-revert-on-exit')?.checked;
    S.turbo = false;
    if (e.error) toast(t('exit-error', { error: e.error }));
    else {
      report(resumedOnly ? t('exit-turbo') : t('exit-undone'), e.outcomes);
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
      toast(t('game-started', { name: D.games[i].name }) + ' ' + (p ? t('game-started-no-auto') : t('game-started-no-profile')) + '.');
      return;
    }
    S.game = i;
    DS('game-target').forEach(s => { s.value = String(i); });
    caps();
    useProfile(e.appid, true);
    apply(false, t('game-started-applied', { name: p.name }));
  }

  async function copyLaunchOptions() {
    const a = game().adapter;
    if (!a) return;
    try {
      await navigator.clipboard.writeText(a.launchOptions);
      toast(t('copied', { options: a.launchOptions, game: game().name }));
    } catch (err) {
      toast(t('err-copy', { error: String(err && err.message ? err.message : err), options: a.launchOptions }));
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
    if (!g) { toast(t('pick-game')); return; }
    if (!api.inTauri) { toast(t('play-only-app')); return; }
    const dry = !!first('dry-run')?.checked;
    /* Se marca antes de lanzar: game-launched puede llegar antes de que vuelva la llamada. */
    S.turboPending = dry ? null : g.appid;
    S.turboRetried = false;
    let running;
    try {
      running = await api.launchGame(g.appid);
    } catch (err) {
      S.turboPending = null;
      toast(t('err-launch', { name: g.name, error: String(err && err.message ? err.message : err) }));
      return;
    }
    g.running = running;
    if (running) {
      S.turboPending = null;
      if (dry) toast(t('dry-running', { name: g.name }));
      else startTurbo(S.game);
      return;
    }
    toast(dry
      ? t('dry-launch', { name: g.name, n: S.chk.filter(Boolean).length })
      : t('launching', { name: g.name }));
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
    await apply(false, t('turbo-verb', { name: g.name }), true);
    if (!S.turbo && retry) S.turboPending = g.appid;
  }

  /* Re-escaneo de Turbo: solo avisa si congeló algo nuevo. */
  function onTurboRescan(e) {
    if (e.error) { toast(t('turbo-rescan-error', { error: e.error })); return; }
    if (e.outcomes && e.outcomes.done) { toast(t('turbo-rescan-done', { n: e.outcomes.done })); syncLimited(); }
  }

  /* ---------- Perfiles por juego (It3.2) ---------- */
  /* No van en el perfil: la clave (juego), el propio perfil, ajustes de la app y lo que se guarda aparte. */
  const PROFILE_SKIP = new Set(['profile', 'profile-auto-apply', 'game-target', 'language', 'overlay-hotkey', 'perf-metric', 'power-plan', 'game-priority']);

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
    setText('[data-setting="apply"] .lbl', first('dry-run')?.checked ? t('apply-dry') : t('apply-label'));
    syncAffinityCores(); renderPower(); render(); sess();
  }

  function fillProfiles(selected) {
    DS('profile').forEach(s => {
      s.innerHTML = `<option value="global">${t('profile-global')}</option>` +
        Object.entries(S.profiles).map(([id, p]) => `<option value="${esc(id)}">${esc(p.name)}</option>`).join('') +
        (D.games.length ? `<option value="nuevo">${t('profile-new')}</option>` : '');
      s.value = selected in S.profiles ? String(selected) : 'global';
    });
  }

  /* El perfil del juego elegido, o Global (los valores del nivel por defecto, como al abrir). */
  function useProfile(appid, quiet) {
    const p = S.profiles[appid];
    loadProfile(p ? p.data : S.base);
    fillProfiles(p ? appid : 'global');
    if (!quiet) toast(p ? t('profile-loaded', { name: p.name }) : t('profile-global-loaded'));
  }

  async function saveProfile() {
    const g = D.games[S.game];
    if (!g) { toast(t('profile-pick-game')); return; }
    const data = profileState();
    try {
      await api.saveProfile(g.appid, g.name, data);
      S.profiles[g.appid] = { name: g.name, data };
      fillProfiles(g.appid);
      toast(t('profile-saved', { name: g.name }) + (first('profile-auto-apply')?.checked ? ' ' + t('profile-saved-auto') : '') + '.');
    } catch (err) {
      toast(t('err-profile-save', { error: String(err && err.message ? err.message : err) }));
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
      if (b.dataset.v === 'normal') { setCheck('game-priority-persistent', false); toast(t('prio-normal-unchecks')); }
    }
  }

  /* ---------- Sensores y FPS (eventos) ---------- */
  const fmtSensor = s => (s.value == null ? t('unavailable') : s.unit === 'c' ? Math.round(s.value) + ' °C' : s.unit === 'pct' ? Math.round(s.value) + ' %' : Math.round(s.value).toLocaleString(I18N.lang) + ' rpm');

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

  /*
   * Cambió el hardware (It6.1): se vuelve a leer el equipo y se rehace lo que depende de él
   * (batería, monitores y Hz, planes, GPU, recomendaciones), sin tocar lo que eligió el usuario
   * si sigue existiendo.
   */
  async function onDeviceChanged(kinds) {
    try {
      const fresh = await api.load();
      const pw0 = D.device.power, planName = pw0 && S.plan != null ? pw0.plans[S.plan] : null;
      const keep = ['gpu-preference', 'resolution-override'].map(id => [id, first(id)?.value]);
      Object.assign(D, { device: fresh.device, recommendations: fresh.recommendations, vbsBlocked: fresh.vbsBlocked });
      const pw = D.device.power;
      const i = pw && planName != null ? pw.plans.indexOf(planName) : -1;
      S.plan = !pw ? null : i >= 0 ? i : pw.forLevel[S.level - 1];
      device();
      keep.forEach(([id, v]) => DS(id).forEach(s => { if (v != null && [...s.options].some(o => o.value === v)) s.value = v; }));
      renderPower();
      toast(t('device-changed', { what: kinds.map(k => t('hw-' + k)).join(', ') }));
    } catch (err) {
      console.warn('releer el equipo:', err);
    }
  }

  /* La ventana se ocultó con el juego vigilado: se avisa fuera de ella. */
  function onBackground() {
    api.notify('FrameRush', t('background-watching')).catch(err => console.warn('notificación:', err));
  }

  /* Alerta (It3.6): en la ventana y como notificación del sistema. */
  function raiseAlert(msg) {
    toast(msg);
    api.notify('FrameRush', msg).catch(err => console.warn('notificación:', err));
  }

  function onSensors(list) {
    S.lastSensors = list;
    const w = +(first('thermal-warn-threshold')?.value || 0), a = +(first('thermal-alert-threshold')?.value || 0);
    const pick = k => list.find(s => s.kind === k && s.value != null) || list.find(s => s.kind === k);
    const fans = list.filter(s => s.kind === 'fan');
    const temps = {};
    renderFans(fans);
    ['cpu', 'gpu'].forEach(k => {
      const s = pick(k), temp = s && s.value != null ? Math.round(s.value) : null;
      const c = temp == null ? null : temp >= a ? 'bad' : temp >= w ? 'warn' : 'ok';
      $$(`[data-temp="${k}"]`).forEach(e => {
        e.className = e.className.replace(/\s*(c-\w+|na)\b/g, '');
        if (temp == null) { e.textContent = t('unavailable'); e.classList.add('na'); } else { e.textContent = temp + ' °C'; e.classList.add('c-' + c); }
      });
      $$(`[data-meter="${k}"]`).forEach(m => { m.style.width = temp == null ? '0' : Math.min(temp, 100) + '%'; m.className = c ? 'bg-' + c : ''; });
      setText(`.${k}Src`, s ? s.source : t('no-sensor'));
      temps[k] = temp;
      /* thermal-alerts: una vez al pasar la de alerta; se vuelve a armar al bajar de la de aviso. */
      if (temp == null || temp < w) S.alerts[k] = false;
      else if (c === 'bad' && first('thermal-alerts')?.checked && !S.alerts[k]) { S.alerts[k] = true; raiseAlert(t('thermal-alert', { part: k.toUpperCase(), temp })); }
    });
    /* fan-stop-alert: un ventilador a 0 rpm con la CPU o la GPU en la temperatura de aviso o más. */
    const hot = ['cpu', 'gpu'].filter(k => temps[k] != null && temps[k] >= w).sort((x, y) => temps[y] - temps[x])[0];
    const stopped = fans.filter(f => f.value === 0);
    if (!hot || !stopped.length) S.alerts.fan = false;
    else if (first('fan-stop-alert')?.checked && !S.alerts.fan) {
      S.alerts.fan = true;
      raiseAlert(t('fan-stop-alert', { fans: stopped.map(f => f.label).join(', '), part: hot.toUpperCase(), temp: temps[hot] }));
    }
    fans.forEach((f, i) => {
      $$(`[data-fan="${i}"]`).forEach(e => {
        e.className = 'v' + (f.value == null ? ' na' : f.value === 0 ? ' c-bad' : '');
        e.innerHTML = f.value === 0 ? t('fan-stopped') : esc(fmtSensor(f)) + (f.target ? ` <span class="na" title="${t('fan-target-title')}">${t('fan-target')}</span>` : '');
      });
      /* Solo hay escala si la fuente da %; las RPM no tienen máximo conocido. */
      $$(`[data-fanbar="${i}"]`).forEach(b => { b.innerHTML = f.unit === 'pct' && f.value != null ? `<i class="bg-ok" style="width:${Math.min(100, f.value)}%"></i>` : ''; });
    });
    $$('.fanCount').forEach(e => {
      const ok = fans.filter(x => x.value != null).length;
      e.innerHTML = fans.length ? `${ok}<span class="na"> / ${fans.length}</span>`
        : `<span class="na">${t('unavailable')}${D.device.laptop ? ' · ' + t('laptop') : ''}</span>`;
    });
    const src = [...new Set(list.map(s => s.source))];
    /* Sin ventiladores se dice por qué: en un portátil (PCSystemType = Mobile) ninguna fuente los expone. */
    const where = D.sensorSources.join(', ') || t('sources-none');
    const admin = D.sensorNeedsAdmin.length && !S.admin
      ? ' ' + t('sensors-need-admin', { list: D.sensorNeedsAdmin.join(', '), button: ELEVATE_LABEL[D.os].replace(/^\S+ /, '') }) : '';
    const noFans = admin || (fans.length ? ''
      : D.device.laptop ? ' ' + t('fans-laptop', { where })
        : ' ' + t('fans-none', { where }));
    DS('sensor-backend-notice').forEach(e => {
      e.textContent = src.length ? t('sensors-from', { list: src.join(', ') }) + (pick('cpu') ? '' : ' ' + t('sensors-no-cpu')) + noFans
        : t('sensors-none') + (D.device.laptop ? ' ' + t('sensors-laptop') : '') + admin;
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
  /* Diagnóstico (It4.5): el backend guarda el informe y la UI dice dónde. */
  async function exportDiagnostics(btn) {
    btn.disabled = true;
    toast(I18N.t('diag-exporting'));
    try { toast(I18N.t('diag-saved', { path: await api.exportDiagnostics() }), 10000); }
    catch (e) { toast(I18N.t('diag-failed', { error: String(e) }), 8000); }
    finally { btn.disabled = false; }
  }

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
      if (!$('#cwAck').checked) { toast(I18N.t('cw-ack-first')); return; }
      DS('compat-warning').forEach(x => { x.hidden = true; }); $('#cwAck').checked = false;
      const cw = S.cwRetry || { verb: I18N.t('verb-apply'), turbo: false }; S.cwRetry = null;
      apply(true, cw.verb, cw.turbo); return;
    }
    if (t.closest('[data-cw]')) { S.cwRetry = null; DS('compat-warning').forEach(x => { x.hidden = true; }); return; }
    if (t.closest('[data-rb]')) { DS('reboot-required').forEach(x => { x.hidden = true; }); return; }
    const fz = t.closest('[data-frz]');
    if (fz) { resumeForeign(fz.dataset.frz); return; }
    const cr = t.closest('[data-crash]');
    if (cr) {
      DS('restore-after-crash').forEach(x => { x.hidden = true; });
      if (cr.dataset.crash === 'restore') revert(); else toast(I18N.t('crash-ignored'));
      return;
    }
    const wb = t.closest('[data-win]');
    if (wb) { if (!api.inTauri) toast(I18N.t('win-only-app')); else api.win(wb.dataset.win); return; }

    const el = t.closest('button[data-setting]'); if (!el) return;
    const id = el.dataset.setting;
    if (id === 'apply') apply();
    if (id === 'undo-last') undo();
    if (id === 'restore') revert();
    if (id === 'launch-game') launchGame();
    if (id === 'status-view') { const v = $('#stv'); v.hidden = !v.hidden; if (!v.hidden) showStatus(v); }
    if (id === 'realtime-confirm') {
      if (!$('#rtAck').checked) { toast(I18N.t('rt-ack-first')); return; }
      S.rt = true; $$('[data-setting="game-priority"] button').forEach(x => x.classList.toggle('on', x.dataset.v === 'realtime'));
      $('#rtBox').hidden = true; $('#rtAck').checked = false; toast(I18N.t('rt-confirmed'));
    }
    if (id === 'run-as-admin') elevate();
    if (id === 'process-refresh') refreshList(el);
    if (id === 'profile-save') saveProfile();
    if (id === 'launch-options-copy') copyLaunchOptions();
    if (id === 'export-diagnostics') exportDiagnostics(el);
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
      toast(I18N.t('game-picked', { name: g.name }) + (g.appid in S.profiles ? ' · ' + I18N.t('profile-loaded-short') : ''));
    }
    if (id === 'cpu-affinity') syncAffinityCores();
    if (id === 'custom-power-plan') renderPower();
    if (id === 'game-priority-persistent' && t.checked && priorityValue() === 'normal') { t.checked = false; toast(I18N.t('persistent-needs-high')); }
    if (id === 'vbs-hvci-off' && t.checked) toast(I18N.t('vbs-confirm-later'));
    if (id === 'fps-cap' && +t.value > 0) toast(I18N.t('fps-cap-set', { value: t.value }));
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
    if (id === 'language') I18N.setLanguage(t.value); /* recarga la ventana con el idioma nuevo */
    if (id === 'overlay-hotkey') setHotkey(t.value, true);
    if (id === 'dry-run') setText('[data-setting="apply"] .lbl', t.checked ? I18N.t('apply-dry') : I18N.t('apply-label'));
    if (id === 'auto-revert-on-exit') sess();
    if (['thermal-alerts', 'thermal-alert-threshold', 'thermal-warn-threshold', 'fan-stop-alert'].includes(id)) S.alerts = {};
  });

  document.addEventListener('keydown', e => { if (e.ctrlKey && e.key === 'Enter') { e.preventDefault(); apply(); } }); /* shortcut-apply */

  /* ---------- Arranque ---------- */
  /* De momento el helper solo lee los sensores que piden admin; aplicar sigue sin conectar. */
  async function elevate() {
    DS('run-as-admin').forEach(b => { b.disabled = true; });
    waitShow('uac');
    try {
      await api.elevate();
      waitHide();
      toast(t('admin-on'));
      await init();
    } catch (err) {
      waitHide();
      toast(t('err-admin', { error: String(err && err.message ? err.message : err) }));
      elevateButton();
    }
  }

  function elevateButton() {
    DS('run-as-admin').forEach(b => { b.disabled = S.admin; b.textContent = S.admin ? t('admin-active') : ELEVATE_LABEL[D.os]; });
  }

  async function init() {
    stopStreams.forEach(f => f()); stopStreams = [];
    waitShow('load');
    try {
      D = await api.load();
    } catch (err) {
      waitHide();
      $('.scroll').innerHTML = `<p class="note" role="alert">${esc(err && err.message ? err.message : err)}</p>`;
      return;
    }
    /* Juegos de Steam del equipo; adaptadores y recomendaciones llegan en M4. */
    D.steamRecs = D.steamRecs || [];
    Object.assign(S, { admin: D.elevated, game: Math.max(0, D.games.findIndex(g => g.running)), hist: [], rt: false, alerts: {}, rb: [], turbo: false, fps: [], lastSensors: null, fanIds: '', released: [], chk: [], stc: D.steamRecs.map(s => !!s.checked) });
    waitHide();
    $$('[data-touched]').forEach(x => { delete x.dataset.touched; });

    DS('game-target').forEach(s => {
      s.innerHTML = D.games.length
        ? D.games.map((g, i) => `<option value="${i}">${esc(g.name)}${g.running ? ' ' + t('game-running') : ''}${g.antiCheat ? ' · ' + esc(g.antiCheat.name) : ''}</option>`).join('')
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
    checkFrozen();
    /* Global = lo de arriba; el perfil del juego en marcha, si lo hay, se carga encima. */
    S.base = profileState();
    try { S.profiles = await api.profiles(); } catch (err) { S.profiles = {}; toast(t('err-profiles', { error: String(err && err.message ? err.message : err) })); }
    const g = D.games[S.game];
    if (g && g.appid in S.profiles) useProfile(g.appid); else fillProfiles('global');
    stopStreams.push(api.onSensors(onSensors), api.onFps(onFps), api.onGameExited(onGameExited), api.onBackground(onBackground), api.onDeviceChanged(onDeviceChanged), api.onGameLaunched(onGameLaunched), api.onTurboRescan(onTurboRescan));
  }

  /* Atajo del overlay (It3.5): de la app, no del perfil; se recuerda en este equipo. */
  const HOTKEY_KEY = 'framerush.overlayHotkey';
  async function setHotkey(accel, announce) {
    try {
      await api.setOverlayHotkey(accel);
      try { localStorage.setItem(HOTKEY_KEY, accel); } catch (_) { /* sin almacenamiento: dura esta sesión */ }
      if (announce) toast(accel.trim() ? I18N.t('hotkey-set', { accel: accel.trim() }) : I18N.t('hotkey-off'));
    } catch (e) {
      toast(I18N.t('hotkey-error', { accel, error: String(e) }), 8000);
    }
  }
  {
    let saved = null;
    try { saved = localStorage.getItem(HOTKEY_KEY); } catch (_) { /* sin almacenamiento */ }
    const input = first('overlay-hotkey');
    if (saved != null && input) input.value = saved;
    if (input) setHotkey(input.value, false);
    api.trayLabels(t('tray-toggle'), t('tray-quit')).catch(err => console.warn('bandeja:', err));
  }

  api.onProgress(onProgress); /* una vez: init() se vuelve a llamar al elevar y al cambiar el equipo */
  DS('hardware-profile').forEach(e => { e.textContent = t('reading-device'); });
  DS('language').forEach(x => { x.value = I18N.lang; });
  init();
})();

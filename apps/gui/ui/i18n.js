/*
 * Textos de la interfaz en es/en con Fluent (It4.6, ajuste `language`). Los mensajes están en
 * locales/<idioma>.ftl; el HTML los pide con data-l10n-id (valor y atributos title/aria-label)
 * y app.js con t(id, args). Lo que falte en un idioma sale del español.
 * El idioma elegido se recuerda en este equipo (localStorage); por defecto, español
 * (design/SETTINGS.md). Cambiarlo recarga la ventana.
 */
(function () {
  'use strict';

  const { FluentBundle, FluentResource } = window.FluentBundle;
  const LANGS = ['es', 'en'];
  const FALLBACK = 'es';
  const KEY = 'framerush.language';

  function saved() {
    try { const v = localStorage.getItem(KEY); return LANGS.includes(v) ? v : FALLBACK; } catch { return FALLBACK; }
  }

  async function bundle(lang) {
    const res = await fetch(`locales/${lang}.ftl`);
    if (!res.ok) throw new Error(`locales/${lang}.ftl: ${res.status}`);
    const b = new FluentBundle(lang, { useIsolating: false });
    const errors = b.addResource(new FluentResource(await res.text()));
    errors.forEach(e => console.warn(`locales/${lang}.ftl:`, e));
    return b;
  }

  const lang = saved();
  let bundles = [];

  function find(id) {
    for (const b of bundles) { const m = b.getMessage(id); if (m) return [b, m]; }
    console.warn('Falta el texto', id);
    return [null, null];
  }

  function format(b, pattern, args) {
    const errors = [];
    const out = b.formatPattern(pattern, args, errors);
    errors.forEach(e => console.warn(e));
    return out;
  }

  /* Texto del mensaje `id`; sin él, el propio id (se ve que falta, no se inventa). */
  function t(id, args) {
    const [b, m] = find(id);
    return b && m.value ? format(b, m.value, args) : id;
  }

  /* Pone los textos de los data-l10n-id de `root`. Un mensaje sin valor solo cambia atributos. */
  function apply(root) {
    root.querySelectorAll('[data-l10n-id]').forEach(el => {
      const [b, m] = find(el.dataset.l10nId);
      if (!b) return;
      if (m.value) el.textContent = format(b, m.value);
      Object.entries(m.attributes).forEach(([k, p]) => el.setAttribute(k, format(b, p)));
    });
  }

  function setLanguage(next) {
    if (!LANGS.includes(next) || next === lang) return;
    try { localStorage.setItem(KEY, next); } catch { /* sin almacenamiento: vale solo esta vez */ }
    location.reload();
  }

  const ready = (async () => {
    const order = lang === FALLBACK ? [lang] : [lang, FALLBACK];
    bundles = await Promise.all(order.map(bundle));
    document.documentElement.lang = lang;
    apply(document);
  })();

  window.FrameRushI18n = { lang, ready, t, apply, setLanguage };
})();

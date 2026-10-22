// Hoppy send window: draws what Rust says (`hoppy.init`, `hoppy.render`; sendui/mod.rs `View`)
// and reports the user's choices through `window.ipc.postMessage` (validated in Rust:
// `sendui::parse_action`). Text is only ever set with textContent: never HTML from data.
'use strict';

(() => {
  const $ = (id) => document.getElementById(id);
  const root = document.documentElement;
  let view = null;
  let lastScreen = '';

  const send = (msg) => {
    if (window.ipc && typeof window.ipc.postMessage === 'function') {
      window.ipc.postMessage(JSON.stringify(msg));
    }
  };

  const el = (tag, cls, text) => {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text !== undefined && text !== null) e.textContent = String(text);
    return e;
  };

  // Focus is kept across re-renders by a key (progress updates redraw the panel).
  const keyOf = (e) => (e && e.dataset ? e.dataset.key : undefined);

  const BADGE_ICON = { verified: '✓', encrypted: '🔒', plain: '⚠' };

  function deviceList(v) {
    const list = el('ul', 'devices');
    list.setAttribute('aria-label', v.list_label);
    v.devices.forEach((d, i) => {
      const li = el('li');
      const b = el('button', 'device');
      b.type = 'button';
      b.dataset.key = 'dev:' + i;
      b.setAttribute('aria-label', d.aria);
      b.tabIndex = i === 0 ? 0 : -1;
      const avatar = el('span', 'avatar', d.initial);
      avatar.setAttribute('aria-hidden', 'true');
      const info = el('span', 'info');
      info.append(el('span', 'name', d.name), el('span', 'platform', d.platform));
      const badge = el('span', 'badge badge-' + d.badge);
      badge.append(el('span', 'badge-icon', BADGE_ICON[d.badge] || ''), el('span', 'badge-text', d.badge_label));
      const go = el('span', 'go', '→');
      [info, badge, go].forEach((x) => x.setAttribute('aria-hidden', 'true'));
      b.append(avatar, info, badge, go);
      b.addEventListener('click', () => {
        if (!b.disabled) {
          list.querySelectorAll('button').forEach((x) => { x.disabled = true; });
          b.classList.add('picked');
          send({ t: 'pick', i });
        }
      });
      li.append(b);
      list.append(li);
    });
    // Arrow keys move between the cards (one Tab stop for the whole list).
    list.addEventListener('keydown', (e) => {
      const cards = [...list.querySelectorAll('button.device')];
      const at = cards.indexOf(document.activeElement);
      if (at < 0) return;
      let to = -1;
      if (e.key === 'ArrowDown' || e.key === 'ArrowRight') to = Math.min(cards.length - 1, at + 1);
      else if (e.key === 'ArrowUp' || e.key === 'ArrowLeft') to = Math.max(0, at - 1);
      else if (e.key === 'Home') to = 0;
      else if (e.key === 'End') to = cards.length - 1;
      if (to >= 0) {
        e.preventDefault();
        cards.forEach((c, j) => { c.tabIndex = j === to ? 0 : -1; });
        cards[to].focus();
      }
    });
    return list;
  }

  function progress(v) {
    const wrap = el('div', 'progress');
    const bar = el('div', 'track');
    bar.setAttribute('role', 'progressbar');
    bar.setAttribute('aria-label', v.title);
    bar.setAttribute('aria-valuemin', '0');
    bar.setAttribute('aria-valuemax', '100');
    const fill = el('div', 'fill');
    bar.append(fill);
    wrap.append(bar);
    const pct = v.progress.pct;
    if (pct === null || pct === undefined) {
      wrap.classList.add('indeterminate');
    } else {
      bar.setAttribute('aria-valuenow', String(pct));
      fill.style.width = pct + '%';
      const label = el('span', 'pct', pct + '%');
      label.setAttribute('aria-hidden', 'true');
      wrap.append(label);
    }
    return wrap;
  }

  function codePanel(c) {
    const box = el('div', 'code code-' + c.state);
    box.setAttribute('role', 'group');
    box.setAttribute('aria-label', c.label);
    box.append(el('p', 'code-intro', c.intro));
    const row = el('div', 'emoji-row');
    c.emoji.forEach((x) => {
      const cell = el('div', 'emoji-cell');
      const pic = el('span', 'emoji', x.emoji);
      pic.setAttribute('role', 'img');
      pic.setAttribute('aria-label', x.name);
      const name = el('span', 'emoji-name', x.name);
      name.setAttribute('aria-hidden', 'true');
      cell.append(pic, name);
      row.append(cell);
    });
    box.append(row);
    if (c.state === 'open' || c.state === 'saving') {
      const actions = el('div', 'code-actions');
      const later = el('button', 'btn', c.later_label);
      later.type = 'button';
      later.dataset.key = 'code:later';
      later.addEventListener('click', () => send({ t: 'later' }));
      const match = el('button', 'btn primary', c.match_label);
      match.type = 'button';
      match.dataset.key = 'code:match';
      match.addEventListener('click', () => send({ t: 'match' }));
      if (c.state === 'saving') { later.disabled = true; match.disabled = true; }
      actions.append(later, match);
      box.append(actions);
    } else if (c.result) {
      box.append(el('p', 'code-result', c.result));
    }
    return box;
  }

  function render(v) {
    if (!v || typeof v !== 'object') return;
    const focusedKey = keyOf(document.activeElement);
    const screenChanged = v.screen !== lastScreen;
    view = v;
    root.dataset.screen = v.screen;
    root.dataset.tone = v.tone;
    root.dataset.busy = v.busy ? '1' : '0';

    const mascot = $('mascot');
    if (mascot.dataset.kind !== v.mascot) mascot.dataset.kind = v.mascot;
    $('decor').textContent = v.decor || '';
    $('header').textContent = v.header;
    $('title').textContent = v.title;

    const body = $('body');
    const parts = [];
    if (v.text) parts.push(el('p', 'text', v.text));
    if (v.tips.length) {
      const tips = el('ol', 'tips');
      v.tips.forEach((t) => tips.append(el('li', null, t)));
      parts.push(tips);
    }
    if (v.devices.length) parts.push(deviceList(v));
    if (v.progress) parts.push(progress(v));
    if (v.note) parts.push(el('p', 'note', v.note));
    if (v.code) parts.push(codePanel(v.code));
    body.replaceChildren(...parts);
    // Long text (key changed, firewall hint) scrolls: then keyboard users can focus and
    // scroll it too (WCAG 2.1.1).
    if (body.scrollHeight > body.clientHeight + 1) {
      body.tabIndex = 0;
    } else {
      body.removeAttribute('tabindex');
    }

    const footer = $('footer');
    const buttons = v.buttons.map((b) => {
      const x = el('button', b.primary ? 'btn primary' : 'btn', b.label);
      x.type = 'button';
      x.dataset.key = 'btn:' + b.action;
      x.addEventListener('click', () => send({ t: b.action }));
      return x;
    });
    footer.replaceChildren(...buttons);

    // Screen readers: announce the new title (and the panel's result) once per change.
    if (screenChanged || (v.code && v.code.result)) {
      const live = [v.title, v.text, v.code && v.code.result].filter(Boolean).join('. ');
      if ($('live').textContent !== live) $('live').textContent = live;
    }

    let target = null;
    if (!screenChanged && focusedKey) target = document.querySelector('[data-key="' + focusedKey + '"]');
    if (!target && (screenChanged || !document.activeElement || document.activeElement === document.body)) {
      target = document.querySelector('button.device') ||
        document.querySelector('.code button.primary') ||
        document.querySelector('.footer .btn.primary') ||
        document.querySelector('.footer .btn');
    }
    if (target && !target.disabled) target.focus({ preventScroll: true });
    if (screenChanged) {
      const panel = $('panel');
      panel.classList.remove('enter');
      void panel.offsetWidth; // restart the entry animation
      panel.classList.add('enter');
    }
    lastScreen = v.screen;
  }

  function init(cfg) {
    if (!cfg || typeof cfg !== 'object') return;
    if (/^[a-z]+$/.test(cfg.theme || '')) root.dataset.theme = cfg.theme;
    if (cfg.lang === 'it' || cfg.lang === 'en') root.lang = cfg.lang;
    if (cfg.title) document.title = cfg.title;
  }

  window.hoppy = Object.freeze({ init, render });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      send({ t: 'escape' });
    }
  });
  // No browser behaviour: context menu, file drops (would navigate), text drags.
  document.addEventListener('contextmenu', (e) => e.preventDefault());
  ['dragover', 'drop', 'dragstart'].forEach((t) => document.addEventListener(t, (e) => e.preventDefault()));
  // Animations rest while the window is hidden or minimised.
  const rest = () => { root.classList.toggle('paused', document.hidden); };
  document.addEventListener('visibilitychange', rest);
  rest();

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', () => send({ t: 'ready' }));
  } else {
    send({ t: 'ready' });
  }
})();

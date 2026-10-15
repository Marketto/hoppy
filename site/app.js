// Hoppy download portal: detects the visitor's OS, checks which packages the latest
// GitHub Release really contains, and fills the page. No dependencies.
(() => {
  "use strict";
  const cfg = window.HOPPY || { repo: "Marketto/hoppy", baseUrl: location.origin };

  // Release asset names are stable across versions (see .github/workflows/release.yml).
  const PLATFORMS = [
    { id: "windows", asset: "Hoppy-Setup-x64.exe", icon: "laptop" },
    { id: "android", asset: "Hoppy-android.apk", icon: "phone" },
    { id: "macos", asset: "Hoppy-macos-universal.dmg", icon: "laptop" },
    { id: "linux", asset: "hoppy_amd64.deb", icon: "desktop" },
    { id: "ios", asset: null, icon: "tablet" }, // will link to TestFlight / App Store
  ];
  // Used when the GitHub API is unreachable (rate limit, offline).
  const DEFAULT_AVAILABLE = ["windows", "android"];

  const T = {
    it: {
      title: "Invia file con un saltello.",
      lead: "Tasto destro → Invia a → Hoppy. Tocchi il dispositivo. Fatto. Tra Windows e Android, sulla stessa Wi-Fi.",
      ctaFallback: "Scegli il tuo sistema",
      download: "Scarica per {name}",
      detected: "Abbiamo riconosciuto {name}. {req}",
      soonDetected: "Hoppy per {name} è in arrivo. Intanto puoi usarlo su Windows e Android.",
      chromeos: "Su Chromebook puoi provare la versione Android.",
      other: "Non è il tuo sistema? Trovi tutte le versioni qui sotto.",
      soon: "In arrivo",
      get: "Scarica",
      suggested: "Il tuo sistema",
      qrTitle: "Sul telefono?",
      qrText: "Inquadra il codice con la fotocamera: si apre questa pagina e trovi l'app giusta.",
      allTitle: "Tutte le versioni",
      howTitle: "Come funziona",
      step1: "Installa Hoppy sui tuoi dispositivi.",
      step2: "Su Windows: tasto destro → Invia a → Hoppy. Su Android: Condividi → Hoppy.",
      step3: "Tocchi il dispositivo vicino. L'altro accetta e il file arriva.",
      warn: "Anteprima: in questa versione il trasferimento non è ancora cifrato. Usa Hoppy solo su reti di cui ti fidi.",
      versionUnknown: "Versione in preparazione",
      version: "Versione {v} · {date}",
      source: "Codice sorgente",
      allReleases: "Tutte le versioni",
      names: { windows: "Windows", android: "Android", macos: "Mac", linux: "Ubuntu", ios: "iPhone e iPad" },
      reqs: {
        windows: "Windows 10 e 11, 64 bit. Aggiunge «Invia a → Hoppy».",
        android: "Android 10 o successivo, telefoni e tablet.",
        macos: "macOS 12 o successivo: Mac Intel dal 2015 e Mac con chip Apple.",
        linux: "Ubuntu 22.04 o successivo, 64 bit.",
        ios: "iPhone e iPad dal 2021 in poi.",
      },
      apkHint: "Apri il file scaricato e consenti l'installazione da questa fonte se il telefono lo chiede.",
      exeHint: "Se Windows mostra un avviso: «Ulteriori informazioni» → «Esegui comunque».",
      musicOn: "Attiva musica",
      musicOff: "Disattiva musica",
    },
    en: {
      title: "Send files with a hop.",
      lead: "Right-click → Send to → Hoppy. Tap the device. Done. Between Windows and Android, on the same Wi-Fi.",
      ctaFallback: "Pick your system",
      download: "Download for {name}",
      detected: "We detected {name}. {req}",
      soonDetected: "Hoppy for {name} is coming soon. Meanwhile it works on Windows and Android.",
      chromeos: "On a Chromebook you can try the Android version.",
      other: "Not your system? All versions are below.",
      soon: "Coming soon",
      get: "Download",
      suggested: "Your system",
      qrTitle: "On your phone?",
      qrText: "Point the camera at the code: this page opens and you get the right app.",
      allTitle: "All versions",
      howTitle: "How it works",
      step1: "Install Hoppy on your devices.",
      step2: "On Windows: right-click → Send to → Hoppy. On Android: Share → Hoppy.",
      step3: "Tap the nearby device. They accept and the file arrives.",
      warn: "Preview: transfers are not encrypted yet in this version. Use Hoppy only on networks you trust.",
      versionUnknown: "First version coming soon",
      version: "Version {v} · {date}",
      source: "Source code",
      allReleases: "All releases",
      names: { windows: "Windows", android: "Android", macos: "Mac", linux: "Ubuntu", ios: "iPhone and iPad" },
      reqs: {
        windows: "Windows 10 and 11, 64-bit. Adds “Send to → Hoppy”.",
        android: "Android 10 or later, phones and tablets.",
        macos: "macOS 12 or later: Intel Macs from 2015 and Apple silicon Macs.",
        linux: "Ubuntu 22.04 or later, 64-bit.",
        ios: "iPhone and iPad from 2021 onwards.",
      },
      apkHint: "Open the downloaded file and allow installs from this source if your phone asks.",
      exeHint: "If Windows shows a warning: “More info” → “Run anyway”.",
      musicOn: "Play music",
      musicOff: "Pause music",
    },
  };

  const ICONS = {
    laptop: '<rect x="8" y="10" width="32" height="21" rx="3"/><path d="M4 36h40l-3 4H7z"/>',
    phone: '<rect x="15" y="5" width="18" height="38" rx="4"/><path d="M21 37h6"/>',
    desktop: '<rect x="6" y="7" width="36" height="25" rx="3"/><path d="M18 41h12M24 32v9"/>',
    tablet: '<rect x="9" y="5" width="30" height="38" rx="4"/><path d="M21 37h6"/>',
  };

  // ---------- OS detection
  function detect() {
    const ua = navigator.userAgent || "";
    const ch = navigator.userAgentData;
    const touchMac = /Macintosh/.test(ua) && navigator.maxTouchPoints > 1; // iPadOS 13+
    let os = "unknown";
    if (/Android/i.test(ua)) os = "android";
    else if (/iPhone|iPad|iPod/i.test(ua) || touchMac) os = "ios";
    else if (/CrOS/.test(ua)) os = "chromeos";
    else if (/Windows/i.test(ua)) os = "windows";
    else if (/Mac OS X|Macintosh/.test(ua)) os = "macos";
    else if (/Linux/i.test(ua)) os = "linux";
    if (ch && ch.platform) {
      const p = ch.platform.toLowerCase();
      if (p.includes("android")) os = "android";
      else if (p.includes("windows")) os = "windows";
      else if (p.includes("chrome os") || p.includes("chromeos")) os = "chromeos";
      else if (p.includes("macos")) os = touchMac ? "ios" : "macos";
      else if (p.includes("linux") && os === "unknown") os = "linux";
    }
    const mobile = os === "android" || os === "ios" || !!(ch && ch.mobile) || /Mobi/i.test(ua);
    return { os, mobile };
  }

  // ---------- language
  function pickLang() {
    try {
      const saved = localStorage.getItem("hoppy-lang");
      if (saved === "it" || saved === "en") return saved;
    } catch (e) { /* storage unavailable */ }
    return (navigator.language || "en").toLowerCase().startsWith("it") ? "it" : "en";
  }
  const fmt = (s, vars) => s.replace(/\{(\w+)\}/g, (_, k) => (vars[k] ?? ""));

  // ---------- release lookup
  async function latestRelease() {
    try {
      const res = await fetch(`https://api.github.com/repos/${cfg.repo}/releases/latest`, {
        headers: { Accept: "application/vnd.github+json" },
      });
      if (res.status === 404) return { none: true };
      if (!res.ok) return null;
      const json = await res.json();
      return { tag: json.tag_name, date: json.published_at, assets: (json.assets || []).map((a) => a.name) };
    } catch (e) {
      return null; // offline or rate-limited: fall back to DEFAULT_AVAILABLE
    }
  }
  const downloadUrl = (asset) => `https://github.com/${cfg.repo}/releases/latest/download/${asset}`;

  // ---------- background music: the `portal` loop from tools/gen_music.py.
  // Off by default and never autoplayed (WCAG 1.4.2): it starts only from a click on the
  // button, or, if the visitor turned it on before, at their first interaction here.
  const music = (() => {
    const KEY = "hoppy-music";
    const btn = document.getElementById("music");
    const AC = window.AudioContext || window.webkitAudioContext;
    let on = false, ctx = null, gain = null, src = null, loading = null, timer = 0;
    const api = {
      label() {
        const t = T[state.lang];
        const name = on ? t.musicOff : t.musicOn;
        btn.setAttribute("aria-label", name);
        btn.title = name;
        btn.setAttribute("aria-pressed", String(on));
      },
    };
    if (!AC || !btn) { if (btn) btn.hidden = true; return api; }

    const save = () => { try { localStorage.setItem(KEY, on ? "on" : "off"); } catch (e) { /* ignore */ } };
    const ramp = (to, secs) => {
      const now = ctx.currentTime;
      gain.gain.cancelScheduledValues(now);
      gain.gain.setValueAtTime(gain.gain.value, now);
      gain.gain.linearRampToValueAtTime(to, now + secs);
    };
    function load() {
      // Vorbis where supported (gapless everywhere), MP3 otherwise; loop end from portal.json.
      loading = loading || (async () => {
        const meta = await (await fetch("music/portal.json")).json();
        const ogg = new Audio().canPlayType('audio/ogg; codecs="vorbis"') !== "";
        const data = await (await fetch(ogg ? meta.files.ogg : meta.files.mp3)).arrayBuffer();
        const buf = await new Promise((ok, ko) => ctx.decodeAudioData(data, ok, ko));
        return { buf, end: Math.min(meta.samples / meta.sample_rate, buf.duration) };
      })();
      loading.catch(() => { loading = null; });
      return loading;
    }
    async function play() {
      if (!ctx) {
        ctx = new AC();
        gain = ctx.createGain();
        gain.gain.value = 0;
        gain.connect(ctx.destination);
      }
      clearTimeout(timer);
      const resumed = ctx.resume();            // must be called inside the user gesture
      btn.classList.add("is-playing");
      try {
        await resumed;
        const { buf, end } = await load();
        if (!on || document.hidden) return;
        if (!src) {
          src = ctx.createBufferSource();
          src.buffer = buf;
          src.loop = true;
          src.loopStart = 0;
          src.loopEnd = end;
          src.connect(gain);
          src.start();
        }
        ramp(0.85, 1.5);
      } catch (e) {
        btn.classList.remove("is-playing");  // offline or undecodable: stay silent
      }
    }
    function pause(secs) {
      btn.classList.remove("is-playing");
      if (!ctx) return;
      ramp(0, secs);
      clearTimeout(timer);
      timer = setTimeout(() => { if (!on || document.hidden) ctx.suspend(); }, secs * 1000 + 60);
    }

    btn.addEventListener("click", () => {
      on = !on;
      save();
      api.label();
      if (on) play(); else pause(0.4);
    });
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) pause(0.2);
      else if (on && ctx) play();
    });

    let remembered = false;
    try { remembered = localStorage.getItem(KEY) === "on"; } catch (e) { /* ignore */ }
    if (remembered) {
      on = true;
      const events = ["pointerdown", "pointerup", "touchend", "keydown"];
      const first = (ev) => {
        if (btn.contains(ev.target)) return;   // the button's own click decides
        if (!on) return done();
        play();
        if (ctx && ctx.state !== "suspended") done();
        else if (ctx) ctx.resume().then(() => { if (ctx.state === "running") done(); });
      };
      const done = () => events.forEach((e) => document.removeEventListener(e, first, true));
      events.forEach((e) => document.addEventListener(e, first, true));
    }
    return api;
  })();

  // ---------- render
  const state = { lang: pickLang(), env: detect(), release: undefined };

  function available(p) {
    if (!p.asset) return false;
    const r = state.release;
    if (r && r.none) return false;
    if (r && r.assets) return r.assets.includes(p.asset);
    return DEFAULT_AVAILABLE.includes(p.id);
  }

  function render() {
    const t = T[state.lang];
    document.documentElement.lang = state.lang;
    document.documentElement.classList.toggle("is-mobile", state.env.mobile);
    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const v = t[el.dataset.i18n];
      if (typeof v === "string") el.textContent = v;
    });
    document.getElementById("lang").textContent = state.lang === "it" ? "EN" : "IT";
    music.label();

    // Hero call to action
    const os = state.env.os === "chromeos" ? "android" : state.env.os;
    const p = PLATFORMS.find((x) => x.id === os);
    const btn = document.getElementById("cta-btn");
    const label = document.getElementById("cta-label");
    const note = document.getElementById("cta-note");
    btn.classList.remove("is-soon");
    btn.removeAttribute("aria-disabled");
    if (p && available(p)) {
      label.textContent = fmt(t.download, { name: t.names[p.id] });
      btn.href = downloadUrl(p.asset);
      const hint = p.id === "android" ? t.apkHint : p.id === "windows" ? t.exeHint : "";
      note.textContent = (state.env.os === "chromeos" ? t.chromeos + " " : "") + fmt(t.detected, { name: t.names[p.id], req: t.reqs[p.id] }) + (hint ? " " + hint : "");
    } else if (p) {
      label.textContent = t.soon + " · " + t.names[p.id];
      btn.href = "#platforms";
      btn.classList.add("is-soon");
      btn.setAttribute("aria-disabled", "true");
      note.textContent = fmt(t.soonDetected, { name: t.names[p.id] });
    } else {
      label.textContent = t.ctaFallback;
      btn.href = "#platforms";
      note.textContent = t.other;
    }

    // Platform grid
    const grid = document.getElementById("grid");
    grid.textContent = "";
    for (const x of PLATFORMS) {
      const li = document.createElement("li");
      li.className = "card" + (x.id === os ? " is-detected" : "");
      const ok = available(x);
      li.innerHTML =
        (x.id === os ? `<span class="tag"></span>` : "") +
        `<svg class="card-icon" viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[x.icon]}</svg>` +
        `<p class="card-name"></p><p class="card-req"></p>` +
        (ok ? `<a class="card-btn"></a>` : `<span class="badge"></span>`);
      if (x.id === os) li.querySelector(".tag").textContent = t.suggested;
      li.querySelector(".card-name").textContent = t.names[x.id];
      li.querySelector(".card-req").textContent = t.reqs[x.id];
      if (ok) {
        const a = li.querySelector(".card-btn");
        a.textContent = t.get;
        a.href = downloadUrl(x.asset);
        a.setAttribute("aria-label", `${t.get} · ${t.names[x.id]}`);
      } else {
        li.querySelector(".badge").textContent = t.soon;
      }
      grid.appendChild(li);
    }

    // Version line
    const v = document.getElementById("version");
    const r = state.release;
    if (r && r.tag) {
      const date = new Date(r.date).toLocaleDateString(state.lang === "it" ? "it-IT" : "en-GB", { day: "numeric", month: "long", year: "numeric" });
      v.textContent = fmt(t.version, { v: r.tag.replace(/^v/, ""), date });
    }
  }

  document.getElementById("lang").addEventListener("click", () => {
    state.lang = state.lang === "it" ? "en" : "it";
    try { localStorage.setItem("hoppy-lang", state.lang); } catch (e) { /* ignore */ }
    render();
  });

  render();
  latestRelease().then((r) => { state.release = r; render(); });
})();

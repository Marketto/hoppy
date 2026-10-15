// "Come funziona" / "How it works": per-OS send and receive flows drawn as small HTML/SVG
// mockups (no screenshots). Loaded before app.js, which calls HoppyHowto.render() on every
// render (language switch, release lookup). Strings mirror the real UI text in
// crates/hoppy-cli/src/gui.rs and apps/android/app/src/main/res/values*/strings.xml.
// Mockups are generic: no OS logos, wallpapers or icons, only Hoppy's own bunny.
(() => {
  "use strict";

  // ---------- tiny builders (all text is ours, never user input)
  const at = (x, y, more = "") => `style="left:${x}%;top:${y}%;${more}"`;
  const glyph = () => '<svg class="m-glyph" viewBox="0 0 24 24"><use href="#hb-glyph"/></svg>';
  const app = (cls = "") => `<span class="m-app ${cls}">${glyph()}</span>`;
  const mascot = (happy) => `<svg class="m-mascot" viewBox="0 0 200 200"><use href="#${happy ? "hb-happy" : "hb-bunny"}"/></svg>`;
  const cursor = (x, y) => `<svg class="m-cursor" ${at(x, y)} viewBox="0 0 16 22"><path d="M1.5 1.5v16.5l4.3-3.9 2.9 6.6 3-1.3-2.9-6.5h5.9z"/></svg>`;
  const thumb = (k) => `<svg class="m-thumb t${k}" viewBox="0 0 40 30"><rect width="40" height="30" rx="3"/><circle cx="${k ? 29 : 11}" cy="10" r="5"/><path d="M0 22q10-7 20 0t20 0v8H0z"/></svg>`;

  function desk(os, body, ring = false) {
    let chrome = "";
    if (os === "windows") {
      chrome = `<div class="m-taskbar"><span class="m-tb-apps"><i></i><i></i><i></i><i></i></span><span class="m-tray">${glyph()}<span>10:42</span></span></div>`;
    } else if (os === "macos") {
      chrome = `<div class="m-menubar"><span class="m-mb-l"><i></i><i></i><i></i></span><span class="m-tray">${glyph()}<span>10:42</span></span></div>`;
    } else {
      chrome = `<div class="m-topbar"><span class="m-mb-l"><i></i></span><span class="m-clock">10:42</span><span class="m-tray">${glyph()}<i></i></span></div><div class="m-dock"><i></i><i></i><i></i></div>`;
    }
    return `<div class="m-desk os-${os}${ring ? " is-ring" : ""}">${chrome}${body}</div>`;
  }

  function win(os, title, inner, x = 6, y = 16, w = 64) {
    const ctl = os === "windows" ? '<span class="m-ctl"><i>–</i><i>□</i><i>×</i></span>'
      : os === "macos" ? '<span class="m-dots"><i></i><i></i><i></i></span>' : '<span class="m-x">×</span>';
    return `<div class="m-win os-${os}" ${at(x, y, `width:${w}%`)}><div class="m-title">${os === "macos" ? ctl : ""}<span>${title}</span>${os !== "macos" ? ctl : ""}</div>${inner}</div>`;
  }

  const files = (L, sel = true) =>
    `<div class="m-files"><span class="m-file${sel ? " is-sel" : ""}">${thumb(0)}<span>foto-mare.jpg</span></span><span class="m-file${sel ? " is-sel" : ""}">${thumb(1)}<span>foto-tramonto.jpg</span></span><span class="m-file">${thumb(2)}<span>${L.notes}</span></span></div>`;

  // items: [label, flags] where flags: "hi" highlighted, "hoppy" bunny icon, "sub" arrow, "head" header, "dev" device row
  function menu(items, x, y, w = 50, cls = "") {
    const rows = items.map(([label, f = ""]) => {
      if (f === "sep") return '<hr>';
      const c = f.split(" ").map((k) => "is-" + k).join(" ");
      const icon = f.includes("hoppy") ? glyph() : f.includes("dev") ? '<svg class="m-dev" viewBox="0 0 16 16"><rect x="4" y="1.5" width="8" height="13" rx="2"/></svg>' : "";
      return `<li class="${c}">${icon}<span>${label}</span>${f.includes("sub") ? "<b>›</b>" : ""}</li>`;
    }).join("");
    return `<ul class="m-menu ${cls}" ${at(x, y, `width:${w}%`)}>${rows}</ul>`;
  }

  const toast = (os, title, body, extra = "", actions = null) =>
    `<div class="m-toast os-${os}"><div class="m-toast-app">${app("sm")}<span>Hoppy</span>${extra}</div><b>${title}</b>${body ? `<span>${body}</span>` : ""}` +
    (actions ? `<div class="m-btns">${actions.map(([b, primary]) => `<span class="m-btn${primary ? " is-primary" : ""}">${b}</span>`).join("")}</div>` : "") + "</div>";

  const dialog = (os, title, body, buttons, x = 14, y = 16, w = 72) =>
    `<div class="m-dialog os-${os}" ${at(x, y, `width:${w}%`)}><div class="m-title"><span>${title}</span>${os === "windows" ? '<span class="m-ctl"><i>×</i></span>' : ""}</div><div class="m-dbody">${body}</div><div class="m-btns">${buttons.map(([b, primary]) => `<span class="m-btn${primary ? " is-primary" : ""}">${b}</span>`).join("")}</div></div>`;

  // ---------- phones
  function phone(os, inner, cls = "") {
    return `<div class="ph os-${os} ${cls}"><div class="ph-scr"><div class="ph-status"><span>10:42</span><span class="ph-ic"><i></i><i></i></span></div>${inner}<span class="ph-home"></span></div></div>`;
  }
  const appHead = (L, happy) => `<div class="pa-head">${mascot(happy)}</div>`;
  const devRow = (name, plat, hi) => `<div class="pa-dev${hi ? " is-hi is-tap" : ""}"><svg class="m-dev" viewBox="0 0 16 16"><rect x="1.5" y="3" width="13" height="9" rx="1.5"/><path d="M5 14.5h6"/></svg><span><b>${name}</b><small>${plat}</small></span></div>`;
  const notif = (L, title, body, actions, tapped = false) =>
    `<div class="pn${tapped ? " is-tap" : ""}"><div class="pn-app">${app("sm")}<span>Hoppy</span></div><b>${title}</b>${body ? `<span>${body}</span>` : ""}${actions ? `<div class="pn-act">${actions.map((a, i) => `<span${i ? "" : ' class="is-tap"'}>${a}</span>`).join("")}</div>` : ""}</div>`;
  const gallery = () => `<div class="pg">${[0, 1, 2, 0, 1, 2, 1, 0, 2, 2, 0, 1].map((k, i) => `<span class="${i < 2 ? "is-sel" : ""}">${thumb(k)}</span>`).join("")}</div>`;
  function shareSheet(L, os) {
    const others = ["Chat", "Email", L.notesApp].map((n) => `<span class="ps-t"><i></i><small>${n}</small></span>`).join("");
    return `${gallery()}<div class="ps-dim"></div><div class="ps os-${os}"><span class="ps-grab"></span><div class="ps-head">${thumb(0)}<span><b>${L.twoPhotos}</b><small>3,4 MB</small></span></div><div class="ps-row"><span class="ps-t is-hi is-tap">${app()}<small>Hoppy</small></span>${others}</div></div>`;
  }

  // ---------- copy (IT / EN), names are fictional
  const C = {
    it: {
      notes: "appunti.txt", pics: "Immagini", notesApp: "Note", twoPhotos: "2 foto", size: "3,4 MB",
      open: "Apri", copy: "Copia", rename: "Rinomina", sendTo: "Invia a", sendWith: "Invia con Hoppy", services: "Servizi", more: "Altre opzioni",
      sendHead: "Invia 2 file (3,4 MB) a:", cancel: "Annulla",
      sending: "Invio a Pixel di Marco… 64%", delivered: "Consegnato a Pixel di Marco ✓", deliveredPc: "Consegnato a PC-Ufficio ✓",
      trayHead: "Hoppy · ricevo come PC-Ufficio", traySend: "Invia file…", trayOpen: "Apri cartella ricevuti", trayQuit: "Esci",
      incomingTitle: "Hoppy — file in arrivo", incoming: "Pixel di Marco (Android) vuole inviarti 2 file, 3,4 MB:", accept: "Accetti?",
      yes: "Sì", no: "No", acceptBtn: "Accetta", declineBtn: "Rifiuta",
      received: "Ricevuti 2 file da Pixel di Marco", receivedPc: "Ricevuti 2 file da PC-Ufficio", clickOpen: "Clicca per aprire la cartella.",
      tapOpen: "foto-mare.jpg, foto-tramonto.jpg",
      phoneIncoming: "PC-Ufficio vuole inviarti 2 file (3,4 MB)", pcIncoming: "Pixel di Marco vuole inviarti 2 file (3,4 MB)",
      nearby: "Vicino a te", sendToPhone: "Invia a", ready: "Pronto a ricevere", readyHint: "I dispositivi vicini ti vedono e possono inviarti file.",
      phoneSending: "Invio a PC-Ufficio · 64%", readyAs: "Pronto a ricevere", savedIn: "Salvati in Download/Hoppy",
      flows: { send: "Invio", recv: "Ricezione" },
    },
    en: {
      notes: "notes.txt", pics: "Pictures", notesApp: "Notes", twoPhotos: "2 photos", size: "3.4 MB",
      open: "Open", copy: "Copy", rename: "Rename", sendTo: "Send to", sendWith: "Send with Hoppy", services: "Services", more: "More options",
      sendHead: "Send 2 file(s) (3.4 MB) to:", cancel: "Cancel",
      sending: "Sending to Pixel di Marco… 64%", delivered: "Delivered to Pixel di Marco ✓", deliveredPc: "Delivered to PC-Ufficio ✓",
      trayHead: "Hoppy · receiving as PC-Ufficio", traySend: "Send files…", trayOpen: "Open received folder", trayQuit: "Quit",
      incomingTitle: "Hoppy — incoming files", incoming: "Pixel di Marco (Android) wants to send you 2 file(s), 3.4 MB:", accept: "Accept?",
      yes: "Yes", no: "No", acceptBtn: "Accept", declineBtn: "Decline",
      received: "Received 2 file(s) from Pixel di Marco", receivedPc: "Received 2 files from PC-Ufficio", clickOpen: "Click to open the folder.",
      tapOpen: "foto-mare.jpg, foto-tramonto.jpg",
      phoneIncoming: "PC-Ufficio wants to send 2 files (3.4 MB)", pcIncoming: "Pixel di Marco wants to send 2 files (3.4 MB)",
      nearby: "Nearby", sendToPhone: "Send to", ready: "Ready to receive", readyHint: "Nearby devices can see you and send you files.",
      phoneSending: "Sending to PC-Ufficio · 64%", readyAs: "Ready to receive", savedIn: "Saved in Downloads/Hoppy",
      flows: { send: "Sending", recv: "Receiving" },
    },
  };

  // Shared screens of the Hoppy phone app (Android today, iPhone planned).
  const S = {
    pickDevice: (L, os) => phone(os, `<div class="pa">${appHead(L)}<p class="pa-h">${L.sendToPhone}</p>${devRow("PC-Ufficio", "Windows", true)}${devRow("Tablet di Anna", "Android")}</div>`),
    progress: (L, os) => phone(os, `<div class="pa">${appHead(L)}<p class="pa-h">${L.phoneSending}</p><div class="pa-bar"><i style="width:64%"></i></div><p class="pa-s">foto-mare.jpg</p></div>`),
    done: (L, os) => phone(os, `<div class="pa is-done">${appHead(L, true)}<p class="pa-h">${L.deliveredPc}</p></div>`),
    home: (L, os) => phone(os, `<div class="pa">${appHead(L)}<div class="pa-sw"><b>${L.ready}</b><span class="sw is-on"><i></i></span></div><p class="pa-s">${L.readyHint}</p><p class="pa-h sm">${L.nearby}</p>${devRow("PC-Ufficio", "Windows")}</div>`),
    ask: (L, os) => phone(os, `<div class="pw"><i></i><i></i></div>${notif(L, L.phoneIncoming, "foto-mare.jpg, foto-tramonto.jpg", [L.acceptBtn, L.declineBtn])}`, "is-lock"),
    got: (L, os, body) => phone(os, `<div class="pw"><i></i><i></i></div>${notif(L, L.receivedPc, body, null, true)}`, "is-lock"),
  };

  // ---------- flows: shot(L) + caption + description, per language
  const step = (shot, cap, alt) => ({ shot, cap, alt });
  const FLOWS = {
    windows: {
      kind: "desk",
      send: [
        step((L) => desk("windows", win("windows", L.pics, files(L), 4, 8, 58) + menu([[L.open], [L.copy], ["sep", "sep"], [L.sendWith, "hi hoppy"], [L.sendTo, "sub"], [L.rename]], 34, 22, 62) + cursor(31.5, 19)),
          { it: "Tasto destro sui file → «Invia con Hoppy». C'è anche con Maiusc + tasto destro o in «Invia a → Hoppy».", en: "Right-click the files → “Send with Hoppy”. It is also under Shift + right-click and in “Send to → Hoppy”." },
          { it: "Esplora file con due foto selezionate e il menu del tasto destro aperto: è evidenziata la voce «Invia con Hoppy», con il coniglietto.", en: "File Explorer with two photos selected and the right-click menu open: “Send with Hoppy”, with the bunny icon, is highlighted." }),
        step((L) => desk("windows", win("windows", L.pics, files(L), 4, 8, 58) + menu([[L.sendHead, "head"], ["Pixel di Marco · Android", "dev hi"], ["PC-Ufficio · Windows", "dev"], ["sep", "sep"], [L.cancel]], 24, 26, 72) + cursor(21.5, 23)),
          { it: "Accanto al puntatore compare un menu con i dispositivi vicini: scegli quello giusto.", en: "A small menu by the pointer lists the nearby devices: pick the right one." },
          { it: "Menu accanto al puntatore con il titolo «Invia 2 file (3,4 MB) a:» ed elencati Pixel di Marco · Android (evidenziato) e PC-Ufficio · Windows.", en: "Menu by the pointer titled “Send 2 file(s) (3.4 MB) to:”, listing Pixel di Marco · Android (highlighted) and PC-Ufficio · Windows." }),
        step((L) => desk("windows", win("windows", L.pics, files(L, false), 6, 10, 56) + `<div class="m-tip">${L.sending}</div>`, true),
          { it: "Mentre il file viaggia, l'icona di Hoppy vicino all'orologio mostra l'avanzamento.", en: "While the file travels, Hoppy's icon by the clock shows the progress." },
          { it: "Barra delle applicazioni: sopra l'icona di Hoppy vicino all'orologio un suggerimento dice «Invio a Pixel di Marco… 64%».", en: "Taskbar: above Hoppy's icon by the clock a tooltip reads “Sending to Pixel di Marco… 64%”." }),
        step((L) => desk("windows", win("windows", L.pics, files(L, false), 4, 6, 52) + toast("windows", L.delivered, "", '<svg class="m-note" viewBox="0 0 24 24"><path d="M9 17.5V6.2l10-2.2v11.3"/><ellipse cx="6.4" cy="17.6" rx="3.4" ry="2.7"/><ellipse cx="16.4" cy="15.4" rx="3.4" ry="2.7"/></svg>')),
          { it: "Una notifica conferma «Consegnato ✓», con un piccolo jingle.", en: "A notification confirms “Delivered ✓”, with a short jingle." },
          { it: "Notifica di Hoppy in basso a destra: «Consegnato a Pixel di Marco ✓», con una nota musicale.", en: "Hoppy notification at the bottom right: “Delivered to Pixel di Marco ✓”, with a music note." }),
      ],
      recv: [
        step((L) => desk("windows", menu([[L.trayHead, "head hoppy"], [L.traySend], [L.trayOpen], ["sep", "sep"], [L.trayQuit]], 40, 30, 56), true),
          { it: "Hoppy aspetta nell'area di notifica, vicino all'orologio, e parte da solo con Windows.", en: "Hoppy waits in the notification area by the clock and starts with Windows." },
          { it: "Icona del coniglietto evidenziata vicino all'orologio, con il suo menu aperto: «Hoppy · ricevo come PC-Ufficio», Invia file, Apri cartella ricevuti, Esci.", en: "The bunny icon by the clock is highlighted, with its menu open: “Hoppy · receiving as PC-Ufficio”, Send files, Open received folder, Quit." }),
        step((L) => desk("windows", dialog("windows", L.incomingTitle, `<p>${L.incoming}</p><ul><li>foto-mare.jpg</li><li>foto-tramonto.jpg</li></ul><p><b>${L.accept}</b></p>`, [[L.yes], [L.no, true]], 14, 8, 72)),
          { it: "Quando qualcuno ti invia dei file, Hoppy chiede prima: Sì o No. No è la scelta predefinita.", en: "When someone sends you files, Hoppy asks first: Yes or No. No is the default." },
          { it: "Finestra «Hoppy — file in arrivo»: Pixel di Marco (Android) vuole inviarti 2 file, 3,4 MB, foto-mare.jpg e foto-tramonto.jpg. Accetti? Pulsanti Sì e No, con No selezionato.", en: "“Hoppy — incoming files” window: Pixel di Marco (Android) wants to send you 2 files, 3.4 MB, foto-mare.jpg and foto-tramonto.jpg. Accept? Yes and No buttons, No selected." }),
        step((L) => desk("windows", toast("windows", L.received, L.clickOpen) + cursor(80, 60)),
          { it: "Fatto: un clic sulla notifica apre la cartella Download\\Hoppy.", en: "Done: one click on the notification opens the Downloads\\Hoppy folder." },
          { it: "Notifica di Hoppy: «Ricevuti 2 file da Pixel di Marco. Clicca per aprire la cartella», con il puntatore sopra.", en: "Hoppy notification: “Received 2 files from Pixel di Marco. Click to open the folder”, with the pointer on it." }),
      ],
    },
    android: {
      kind: "phone",
      send: [
        step((L) => phone("android", shareSheet(L, "android")),
          { it: "Da qualunque app tocca Condividi: Hoppy è nella prima fila.", en: "In any app tap Share: Hoppy is in the top row." },
          { it: "Galleria con due foto selezionate e il pannello Condividi aperto: Hoppy, con il coniglietto, è il primo della fila.", en: "Gallery with two photos selected and the Share panel open: Hoppy, with the bunny icon, comes first in the row." }),
        step((L) => S.pickDevice(L, "android"),
          { it: "Hoppy mostra i dispositivi vicini: tocca quello a cui inviare.", en: "Hoppy shows the nearby devices: tap the one to send to." },
          { it: "Schermata di Hoppy con la mascotte e l'elenco «Invia a»: PC-Ufficio · Windows (toccato) e Tablet di Anna · Android.", en: "Hoppy screen with the mascot and the “Send to” list: PC-Ufficio · Windows (tapped) and Tablet di Anna · Android." }),
        step((L) => S.progress(L, "android"),
          { it: "L'altro accetta e il file salta: vedi l'avanzamento.", en: "They accept and the file hops over: you see the progress." },
          { it: "Hoppy mostra «Invio a PC-Ufficio · 64%» con una barra di avanzamento.", en: "Hoppy shows “Sending to PC-Ufficio · 64%” with a progress bar." }),
        step((L) => S.done(L, "android"),
          { it: "«Consegnato!» e Hoppy fa festa.", en: "“Delivered!” and Hoppy celebrates." },
          { it: "La mascotte sorride: «Consegnato a PC-Ufficio ✓».", en: "The mascot smiles: “Delivered to PC-Ufficio ✓”." }),
      ],
      recv: [
        step((L) => S.home(L, "android"),
          { it: "Apri Hoppy e lascia attivo «Pronto a ricevere»: i dispositivi vicini ti vedono.", en: "Open Hoppy and keep “Ready to receive” on: nearby devices can see you." },
          { it: "Schermata di Hoppy con l'interruttore «Pronto a ricevere» acceso e la scritta «I dispositivi vicini ti vedono e possono inviarti file».", en: "Hoppy screen with the “Ready to receive” switch on and the text “Nearby devices can see you and send you files”." }),
        step((L) => S.ask(L, "android"),
          { it: "Arriva una notifica: tocca Accetta o Rifiuta.", en: "A notification arrives: tap Accept or Decline." },
          { it: "Notifica di Hoppy: «PC-Ufficio vuole inviarti 2 file (3,4 MB)», con i pulsanti Accetta e Rifiuta.", en: "Hoppy notification: “PC-Ufficio wants to send 2 files (3.4 MB)”, with Accept and Decline buttons." }),
        step((L) => S.got(L, "android", L.tapOpen),
          { it: "File ricevuti: tocca la notifica per aprirli.", en: "Files received: tap the notification to open them." },
          { it: "Notifica di Hoppy: «Ricevuti 2 file da PC-Ufficio», foto-mare.jpg e foto-tramonto.jpg.", en: "Hoppy notification: “Received 2 files from PC-Ufficio”, foto-mare.jpg and foto-tramonto.jpg." }),
      ],
    },
    macos: {
      kind: "desk",
      send: [
        step((L) => desk("macos", win("macos", L.pics, files(L), 3, 14, 50) + menu([[L.open], [L.copy], ["sep", "sep"], [L.services, "sub hi"]], 16, 36, 39) + menu([[L.sendWith, "hi hoppy"]], 49, 58, 50) + cursor(58, 65)),
          { it: "Nel Finder, tasto destro sui file → Servizi → «Invia con Hoppy». Puoi anche trascinarli sull'icona di Hoppy.", en: "In the Finder, right-click the files → Services → “Send with Hoppy”. You can also drag them onto Hoppy's icon." },
          { it: "Finestra del Finder con due foto selezionate, menu del tasto destro su Servizi e la voce «Invia con Hoppy» evidenziata.", en: "Finder window with two photos selected, the right-click menu on Services and “Send with Hoppy” highlighted." }),
        step((L) => desk("macos", win("macos", L.pics, files(L, false), 4, 22, 50) + menu([[L.sendHead, "head"], ["Pixel di Marco · Android", "dev hi"], ["PC-Ufficio · Windows", "dev"], ["sep", "sep"], [L.cancel]], 26, 32, 66) + cursor(23.5, 29)),
          { it: "Accanto al puntatore compare un menu con i dispositivi vicini: scegli quello giusto.", en: "A small menu by the pointer lists the nearby devices: pick the right one." },
          { it: "Menu accanto al puntatore con il titolo «Invia 2 file (3,4 MB) a:» ed elencati Pixel di Marco · Android (evidenziato) e PC-Ufficio · Windows.", en: "Menu by the pointer titled “Send 2 file(s) (3.4 MB) to:”, listing Pixel di Marco · Android (highlighted) and PC-Ufficio · Windows." }),
        step((L) => desk("macos", win("macos", L.pics, files(L, false), 4, 22, 50) + toast("macos", L.delivered, "", '<svg class="m-note" viewBox="0 0 24 24"><path d="M9 17.5V6.2l10-2.2v11.3"/><ellipse cx="6.4" cy="17.6" rx="3.4" ry="2.7"/><ellipse cx="16.4" cy="15.4" rx="3.4" ry="2.7"/></svg>')),
          { it: "Una notifica conferma «Consegnato ✓», con un piccolo jingle.", en: "A notification confirms “Delivered ✓”, with a short jingle." },
          { it: "Notifica di Hoppy in alto a destra: «Consegnato a Pixel di Marco ✓», con una nota musicale.", en: "Hoppy notification at the top right: “Delivered to Pixel di Marco ✓”, with a music note." }),
      ],
      recv: [
        step((L) => desk("macos", menu([[L.trayHead, "head hoppy"], [L.traySend], [L.trayOpen], ["sep", "sep"], [L.trayQuit]], 40, 11, 58), true),
          { it: "Il coniglietto nella barra dei menu, in alto a destra, riceve per te e si riapre a ogni accesso.", en: "The bunny in the menu bar, at the top right, receives for you and opens again at every login." },
          { it: "Coniglietto evidenziato nella barra dei menu, con il suo menu aperto: «Hoppy · ricevo come PC-Ufficio», Invia file, Apri cartella ricevuti, Esci.", en: "The bunny is highlighted in the menu bar, with its menu open: “Hoppy · receiving as PC-Ufficio”, Send files, Open received folder, Quit." }),
        step((L) => desk("macos", dialog("macos", L.incomingTitle, `<p>${L.incoming}</p><ul><li>foto-mare.jpg</li><li>foto-tramonto.jpg</li></ul>`, [[L.acceptBtn], [L.declineBtn, true]], 18, 14, 68)),
          { it: "Quando qualcuno ti invia dei file, Hoppy chiede prima: Rifiuta è la scelta predefinita (Invio).", en: "When someone sends you files, Hoppy asks first: Decline is the default (Return)." },
          { it: "Finestra «Hoppy — file in arrivo»: Pixel di Marco (Android) vuole inviarti 2 file, 3,4 MB, foto-mare.jpg e foto-tramonto.jpg. Pulsanti Accetta e Rifiuta, con Rifiuta selezionato.", en: "“Hoppy — incoming files” window: Pixel di Marco (Android) wants to send you 2 files, 3.4 MB, foto-mare.jpg and foto-tramonto.jpg. Accept and Decline buttons, Decline selected." }),
        step((L) => desk("macos", toast("macos", L.received, L.clickOpen) + cursor(80, 30)),
          { it: "Fatto: un clic sulla notifica apre la cartella Download/Hoppy.", en: "Done: one click on the notification opens the Downloads/Hoppy folder." },
          { it: "Notifica di Hoppy: «Ricevuti 2 file da Pixel di Marco. Clicca per aprire la cartella», con il puntatore sopra.", en: "Hoppy notification: “Received 2 files from Pixel di Marco. Click to open the folder”, with the pointer on it." }),
      ],
    },
    linux: {
      kind: "desk",
      send: [
        step((L) => desk("linux", win("linux", L.pics, files(L), 13, 13, 52) + menu([[L.open], [L.copy], ["sep", "sep"], [L.sendWith, "hi hoppy"], [L.rename]], 40, 30, 56) + cursor(37.5, 26.5)),
          { it: "In File, tasto destro sui file → «Invia con Hoppy».", en: "In Files, right-click the files → “Send with Hoppy”." },
          { it: "Gestore file con due foto selezionate e il menu del tasto destro aperto: «Invia con Hoppy» è evidenziata.", en: "File manager with two photos selected and the right-click menu open: “Send with Hoppy” is highlighted." }),
        step((L) => desk("linux", dialog("linux", L.sendHead, `<div class="m-list"><span class="is-hi">Pixel di Marco · Android</span><span>PC-Ufficio · Windows</span></div>`, [[L.cancel]], 18, 16, 68) + cursor(66, 37)),
          { it: "Una finestra elenca i dispositivi vicini: scegline uno.", en: "A window lists the nearby devices: pick one." },
          { it: "Finestra «Invia 2 file (3,4 MB) a:» con Pixel di Marco · Android (evidenziato) e PC-Ufficio · Windows.", en: "“Send 2 file(s) (3.4 MB) to:” window with Pixel di Marco · Android (highlighted) and PC-Ufficio · Windows." }),
        step((L) => desk("linux", win("linux", L.pics, files(L, false), 14, 30, 60) + toast("linux", L.delivered, "")),
          { it: "Una notifica conferma la consegna.", en: "A notification confirms the delivery." },
          { it: "Notifica in alto: «Consegnato a Pixel di Marco ✓».", en: "Notification at the top: “Delivered to Pixel di Marco ✓”." }),
      ],
      recv: [
        step((L) => desk("linux", menu([[L.trayHead, "head hoppy"], [L.traySend], [L.trayOpen], ["sep", "sep"], [L.trayQuit]], 40, 11, 58), true),
          { it: "L'icona di Hoppy nella barra in alto ti tiene visibile ai dispositivi vicini.", en: "Hoppy's icon in the top bar keeps you visible to nearby devices." },
          { it: "Icona del coniglietto evidenziata nella barra in alto, con il menu «Hoppy · ricevo come PC-Ufficio».", en: "The bunny icon is highlighted in the top bar, with the “Hoppy · receiving as PC-Ufficio” menu." }),
        step((L) => desk("linux", dialog("linux", L.incomingTitle, `<p>${L.incoming}</p><ul><li>foto-mare.jpg</li><li>foto-tramonto.jpg</li></ul>`, [[L.declineBtn], [L.acceptBtn]], 18, 14, 68)),
          { it: "Una finestra chiede se accetti: Rifiuta è la scelta predefinita.", en: "A window asks whether to accept: Decline is the default." },
          { it: "Finestra «Hoppy — file in arrivo»: Pixel di Marco vuole inviarti 2 file, con i pulsanti Rifiuta e Accetta.", en: "“Hoppy — incoming files” window: Pixel di Marco wants to send you 2 files, with Decline and Accept buttons." }),
        step((L) => desk("linux", toast("linux", L.received, L.clickOpen) + cursor(56, 22)),
          { it: "Fatto: un clic sulla notifica apre la cartella.", en: "Done: one click on the notification opens the folder." },
          { it: "Notifica in alto: «Ricevuti 2 file da Pixel di Marco. Clicca per aprire la cartella».", en: "Notification at the top: “Received 2 files from Pixel di Marco. Click to open the folder”." }),
      ],
    },
    ios: {
      kind: "phone",
      send: [
        step((L) => phone("ios", shareSheet(L, "ios")),
          { it: "Da Foto o da File tocca Condividi → Hoppy.", en: "In Photos or Files tap Share → Hoppy." },
          { it: "Foto con due immagini selezionate e il foglio Condividi aperto: Hoppy è il primo della fila.", en: "Photos with two pictures selected and the Share sheet open: Hoppy comes first in the row." }),
        step((L) => S.pickDevice(L, "ios"),
          { it: "Tocca il dispositivo vicino.", en: "Tap the nearby device." },
          { it: "Schermata di Hoppy con la mascotte e l'elenco «Invia a»: PC-Ufficio · Windows (toccato) e Tablet di Anna · Android.", en: "Hoppy screen with the mascot and the “Send to” list: PC-Ufficio · Windows (tapped) and Tablet di Anna · Android." }),
        step((L) => S.done(L, "ios"),
          { it: "«Consegnato!» e Hoppy fa festa.", en: "“Delivered!” and Hoppy celebrates." },
          { it: "La mascotte sorride: «Consegnato a PC-Ufficio ✓».", en: "The mascot smiles: “Delivered to PC-Ufficio ✓”." }),
      ],
      recv: [
        step((L) => S.home(L, "ios"),
          { it: "Apri Hoppy: mentre è aperto, i dispositivi vicini ti vedono.", en: "Open Hoppy: while it is open, nearby devices can see you." },
          { it: "Schermata di Hoppy con l'interruttore «Pronto a ricevere» acceso.", en: "Hoppy screen with the “Ready to receive” switch on." }),
        step((L) => S.ask(L, "ios"),
          { it: "Una notifica chiede se accetti i file.", en: "A notification asks whether to accept the files." },
          { it: "Notifica di Hoppy: «PC-Ufficio vuole inviarti 2 file (3,4 MB)», con Accetta e Rifiuta.", en: "Hoppy notification: “PC-Ufficio wants to send 2 files (3.4 MB)”, with Accept and Decline." }),
        step((L) => S.got(L, "ios", L.lang === "it" ? "Li trovi in File → Hoppy." : "Find them in Files → Hoppy."),
          { it: "I file finiscono in File → Hoppy.", en: "The files land in Files → Hoppy." },
          { it: "Notifica di Hoppy: «Ricevuti 2 file da PC-Ufficio», li trovi in File → Hoppy.", en: "Hoppy notification: “Received 2 files from PC-Ufficio”, find them in Files → Hoppy." }),
      ],
    },
  };
  C.it.lang = "it";
  C.en.lang = "en";

  // ---------- render into #os-tabs / #howto-panel
  const ORDER = ["windows", "android", "macos", "linux", "ios"];
  let current = null;

  function render(o) {
    // o: { lang, os (detected), names, soon, isLive(id), icons, t }
    if (!current) current = ORDER.includes(o.os) ? o.os : o.os === "chromeos" ? "android" : "windows";
    const L = C[o.lang];
    const tabs = document.getElementById("os-tabs");
    const panel = document.getElementById("howto-panel");
    const lead = document.getElementById("howto-lead");
    if (!tabs || !panel) return;

    tabs.setAttribute("aria-label", o.t.howTabs);
    tabs.textContent = "";
    for (const id of ORDER) {
      const b = document.createElement("button");
      b.type = "button";
      b.id = "tab-" + id;
      b.className = "os-tab";
      b.setAttribute("role", "tab");
      b.setAttribute("aria-controls", "howto-panel");
      b.setAttribute("aria-selected", String(id === current));
      b.tabIndex = id === current ? 0 : -1;
      b.innerHTML = `<svg viewBox="0 0 48 48" fill="none" stroke="currentColor" stroke-width="3.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${o.icons[id]}</svg><span></span>` +
        (o.isLive(id) ? "" : '<small class="os-soon"></small>');
      b.querySelector("span").textContent = o.names[id];
      if (!o.isLive(id)) b.querySelector(".os-soon").textContent = o.soon;
      b.addEventListener("click", () => { current = id; render(o); document.getElementById("tab-" + id).focus(); });
      tabs.appendChild(b);
    }
    panel.setAttribute("aria-labelledby", "tab-" + current);

    const f = FLOWS[current];
    const live = o.isLive(current);
    lead.textContent = (live ? o.t.howLead : o.t.howLeadSoon).replace("{name}", o.names[current]);
    let html = "";
    for (const key of ["send", "recv"]) {
      const steps = f[key];
      html += `<div class="flow"><h3>${L.flows[key]}${live ? "" : `<span class="badge">${o.soon}</span>`}</h3><ol class="shots is-${f.kind}" style="--n:${steps.length}">`;
      steps.forEach((s, i) => {
        html += `<li style="--i:${i}"><figure><div class="m is-${f.kind}" role="img" aria-label="${s.alt[o.lang].replace(/"/g, "&quot;")}"><div class="mi" aria-hidden="true">${s.shot(L)}</div></div>` +
          `<figcaption><span class="n" aria-hidden="true">${i + 1}</span><span>${s.cap[o.lang]}</span></figcaption></figure></li>`;
      });
      html += "</ol></div>";
    }
    panel.innerHTML = html;
  }

  // Arrow keys move between OS tabs (WAI-ARIA tabs pattern, automatic activation).
  document.addEventListener("keydown", (e) => {
    const tab = e.target.closest && e.target.closest(".os-tab");
    if (!tab) return;
    const i = ORDER.indexOf(tab.id.slice(4));
    const next = { ArrowRight: i + 1, ArrowDown: i + 1, ArrowLeft: i - 1, ArrowUp: i - 1, Home: 0, End: ORDER.length - 1 }[e.key];
    if (next === undefined) return;
    e.preventDefault();
    document.getElementById("tab-" + ORDER[(next + ORDER.length) % ORDER.length]).click();
  });

  window.HoppyHowto = { render };
})();

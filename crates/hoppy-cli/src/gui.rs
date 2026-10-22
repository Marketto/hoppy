//! `hoppyw`: the desktop tray receiver and the "Send with Hoppy" / "Send to → Hoppy" GUI,
//! for Windows (`platform::win`), Linux (`crate::linux::ui`) and macOS (`crate::macos::ui`),
//! through `platform::ui`.
//!
//! - `hoppyw` (no args): single instance, notification-area icon, receives into
//!   Downloads\Hoppy asking Yes/No for each offer, balloon when done. Linux: panel icon
//!   (StatusNotifierItem) with a persistent menu (`Tray::set_menu`, picks arrive as
//!   `TrayEvent::Menu`), notifications as balloons, Accept/Decline buttons on the offer
//!   notification, and the per-user right-click entries + autostart written on the first
//!   run of each version (`platform::first_run_integration`). macOS: menu-bar icon whose
//!   menu pops at each click (like Windows), an "Open at login" switch
//!   (`platform::login_item`), files from the Finder service / app icon handed to
//!   `hoppyw send` by the backend.
//! - `hoppyw open [FILE…]` (Linux launcher): files → `send`, none → the receiver.
//! - `hoppyw send FILE…`: the themed send window (`sendui`, ADR 0011: device cards, progress,
//!   inline verification, results; theme from the tray's "Theme" submenu, `crate::theme`).
//!   If the webview cannot start, the native flow (`send_native`): a popup menu at the
//!   cursor, progress in the tray tooltip, final balloon, then exit. Silent while searching
//!   and sending; on delivery plays the done jingle unless muted (`music` in device.json,
//!   "Sound when delivered"). The receiver never plays it (its balloon has the system sound). Explorer's "Send with Hoppy" verb starts one instance
//!   per selected file: the first collects the others' files (`platform::gather_send_files`).
//!   Without files (desktop shortcut double-clicked) it opens the file picker first.
//! - Updates: the tray checks GitHub Releases 30 s after start, every 3 h, after a wake-up
//!   from sleep and when its menu opens (those two at most every 10 min; `update.rs`), and
//!   at once from "Check for updates" (always answered with a balloon);
//!   "Update to X…" downloads this platform's package, verifies SHA256SUMS.txt and installs
//!   it (`platform::fetch_update`): Windows runs the installer silently and exits (the
//!   installer restarts the tray); macOS replaces /Applications/Hoppy.app and Linux installs
//!   the .deb through pkexec, then a helper restarts the tray (`unix::selfupdate`). Copies
//!   that cannot update themselves only link to the portal.
//! - Device verification (protocol v1, spec §7): the accept question shows the 4-emoji code
//!   with the emoji names and "They match, accept"; the sender shows the same code while the
//!   receiver decides ("They match" / "Later"); people compare the emoji by eye; a changed key is a blocking
//!   warning with "Forget device"; "Trusted devices…" lists remembered devices. The dialogs
//!   are `verify::DialogView`s built here and drawn by `ui::show_dialog` / `ui::ask_offer`.
//! - Firewall (`firewall.rs`): the tray checks at start whether other devices can reach it;
//!   if not, a balloon (once per version) and a menu item offer to add the rules (UAC →
//!   `hoppyw firewall --apply`) or to set a Public network to Private.
//! - `hoppyw firewall --apply|--remove`: no UI, exit code only (started elevated by the tray
//!   and the installer).
//!
//! Safe Rust only: every OS call goes through `platform::ui` (Win32 on Windows, D-Bus +
//! zenity on Linux), so this module is compiled, linted and tested on Linux. Worker threads
//! never touch the UI; they update shared state and call `Waker::wake`, which the UI thread
//! sees as `TrayEvent::Wake`. `ui::NATIVE_POPUPS` (false on Linux) switches the two places
//! where the UIs differ: popup menus vs a persistent tray menu, and menus vs dialogs that
//! have their own Cancel button.
//!
//! The UI thread never does file, registry or network I/O and never waits on a lock that
//! is held across I/O: device.json is cached in `Prefs` (re-read by a worker on each menu
//! open), and opening folders, starting processes, UAC prompts and config writes all run on
//! workers. Handlers are timed (`diag::UiWatch`); see `diag.rs` for the log.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Result, bail};
use hoppy_core::protocol::TRANSFER_PORT;
use hoppy_core::transfer::serve;
use hoppy_core::{
    Discovery, Error, Local, Offer, Peer, PeerKey, Progress, ReceiveHandler, SendOutcome,
    SessionInfo, Trust, TrustEntry, TrustState, TrustStore, Warning, accept_callbacks,
    send_to_peer,
};

use crate::firewall::{self, Verdict};
use crate::platform::ui::{self, Tray, Waker};
use crate::platform::{self, ElevationDeclined, HttpSink, MenuItem, TooManyFiles, TrayEvent};
use crate::sendui::{self, DeviceCard};
use crate::theme::{self, SystemLook, Theme};
use crate::verify::{CodeView, DialogLive, DialogView};
use crate::{
    DialogGuard, SHOW_TEXT_MAX, bind_receiver, config, default_receive_dir, diag,
    display_file_name, display_name, display_platform, display_text, human, start_discovery,
    update,
};

/// Window titles double as identities: `signal_existing` looks for the tray one.
pub(crate) const TRAY_WINDOW: &str = "Hoppy tray";
const SEND_WINDOW: &str = "Hoppy send";
const INSTANCE_MUTEX: &str = r"Local\HoppyTray";

/// Peer search: give up after `SEARCH_MAX`, or `SEARCH_SETTLE` after the first answer.
pub const SEARCH_MAX: Duration = Duration::from_millis(2500);
pub const SEARCH_SETTLE: Duration = Duration::from_millis(800);
/// Keep the icon (and its balloon) alive this long after a send finishes.
const EXIT_AFTER_MS: u32 = 7000;
const EXIT_TIMER: usize = 1;

/// Every user-visible `hoppyw` string, Italian + English. Placeholders: `{name}` etc.,
/// filled by `fill` in a single pass.
pub struct Strings {
    pub tip_ready: &'static str,
    pub menu_header: &'static str,
    pub menu_send: &'static str,
    pub menu_open: &'static str,
    pub menu_music: &'static str,
    pub menu_quit: &'static str,
    pub pick_title: &'static str,
    pub already_running: &'static str,
    pub received_title: &'static str,
    pub received_text: &'static str,
    pub receive_failed: &'static str,
    pub incoming_title: &'static str,
    pub incoming_body: &'static str,
    pub incoming_more: &'static str,
    pub searching: &'static str,
    pub send_header: &'static str,
    pub send_header_more: &'static str,
    pub no_devices: &'static str,
    pub retry: &'static str,
    pub cancel: &'static str,
    pub waiting: &'static str,
    pub sending: &'static str,
    pub delivered: &'static str,
    pub declined: &'static str,
    pub send_failed: &'static str,
    pub fallback: &'static str,
    pub unreachable_title: &'static str,
    pub unreachable_text: &'static str,
    pub cancel_send: &'static str,
    pub close: &'static str,
    pub not_a_file: &'static str,
    pub no_files: &'static str,
    pub too_many_files: &'static str,
    pub usage: &'static str,
    pub update_ready: &'static str,
    pub update_portal: &'static str,
    pub menu_update: &'static str,
    pub menu_download: &'static str,
    pub update_progress: &'static str,
    pub update_failed: &'static str,
    pub update_mismatch: &'static str,
    pub fw_title: &'static str,
    pub fw_text: &'static str,
    pub public_title: &'static str,
    pub public_text: &'static str,
    pub menu_firewall: &'static str,
    pub menu_network: &'static str,
    pub fw_done: &'static str,
    pub fw_failed: &'static str,
    pub fw_policy: &'static str,
    pub menu_classic: &'static str,
    pub menu_login: &'static str,
    pub login_failed: &'static str,
    pub menu_about: &'static str,
    pub restart_explorer: &'static str,
    pub classic_failed: &'static str,
    pub about_failed: &'static str,
    pub accept: &'static str,
    pub decline: &'static str,
    pub no_dialog_tool: &'static str,
    // Protocol v1 security: verification dialogs (spec §7.5–7.7, `verify.rs`).
    /// Emoji names in Italian (`Sas::words`).
    pub italian: bool,
    pub sec_code: &'static str,
    pub sec_verified: &'static str,
    pub sec_plain: &'static str,
    pub sec_new: &'static str,
    pub sec_known: &'static str,
    pub match_accept: &'static str,
    pub match_only: &'static str,
    pub verify_details: &'static str,
    pub later: &'static str,
    pub code_title: &'static str,
    pub code_text: &'static str,
    pub waiting_code: &'static str,
    pub waiting_plain: &'static str,
    pub key_changed_title: &'static str,
    pub key_changed_text: &'static str,
    pub forget_device: &'static str,
    pub forgotten: &'static str,
    pub forget_failed: &'static str,
    pub plain_refused_title: &'static str,
    pub verified_title: &'static str,
    pub sec_name_clash: &'static str,
    pub aborts_title: &'static str,
    pub aborts_text: &'static str,
    pub trust_problem: &'static str,
    pub menu_trusted: &'static str,
    pub trusted_header: &'static str,
    pub trusted_none: &'static str,
    pub trusted_none_text: &'static str,
    pub trusted_verified: &'static str,
    pub trusted_unverified: &'static str,
    pub forget_title: &'static str,
    pub forget_text: &'static str,
    pub forget: &'static str,
    pub dev_verified: &'static str,
    pub dev_encrypted: &'static str,
    pub dev_plain: &'static str,
    pub update_installing: &'static str,
    pub update_cancelled: &'static str,
    pub update_opened: &'static str,
    pub menu_check_updates: &'static str,
    pub about_version: &'static str,
    pub about_dev: &'static str,
    pub about_author: &'static str,
    pub about_github: &'static str,
    pub about_diagnostics: &'static str,
    pub update_current: &'static str,
    pub update_found: &'static str,
    pub update_check_failed: &'static str,
    // The send window (`sendui`, ADR 0011).
    pub ui_window_title: &'static str,
    pub ui_header_one: &'static str,
    pub ui_header_more: &'static str,
    pub ui_searching: &'static str,
    pub ui_searching_text: &'static str,
    pub ui_pick: &'static str,
    pub ui_devices: &'static str,
    pub ui_none_title: &'static str,
    pub ui_none_text: &'static str,
    pub ui_tip_open: &'static str,
    pub ui_tip_wifi: &'static str,
    pub ui_tip_firewall: &'static str,
    pub ui_badge_verified: &'static str,
    pub ui_badge_encrypted: &'static str,
    pub ui_badge_plain: &'static str,
    pub ui_send_to: &'static str,
    pub ui_sending: &'static str,
    pub ui_done_text: &'static str,
    pub ui_declined_text: &'static str,
    pub ui_retry: &'static str,
    pub ui_verify: &'static str,
    pub ui_code: &'static str,
    pub menu_theme: &'static str,
}

pub const IT: Strings = Strings {
    tip_ready: "Hoppy · {name}",
    menu_header: "Hoppy · ricevo come {name}",
    menu_send: "Invia file…",
    menu_open: "Apri cartella ricevuti",
    menu_music: "♪ Suono a fine invio",
    menu_quit: "Esci",
    pick_title: "Hoppy — scegli i file da inviare",
    already_running: "Hoppy è già attivo: trovi l'icona vicino all'orologio.",
    received_title: "Ricevuti {n} file da {name}",
    received_text: "Clicca per aprire la cartella.",
    receive_failed: "Ricezione da {name} non riuscita",
    incoming_title: "Hoppy — file in arrivo",
    incoming_body: "{name} ({platform}, {ip}) vuole inviarti {n} file, {size}:",
    incoming_more: "… e altri {n}",
    searching: "Cerco dispositivi…",
    send_header: "Invia «{file}» ({size}) a:",
    send_header_more: "Invia «{file}» +{more} ({size}) a:",
    no_devices: "Nessun dispositivo trovato — apri Hoppy sull'altro dispositivo (stessa Wi-Fi)",
    retry: "Cerca di nuovo",
    cancel: "Annulla",
    waiting: "In attesa che {name} accetti…",
    sending: "Invio a {name}… {pct}%",
    delivered: "Consegnato a {name} ✓",
    declined: "{name} ha rifiutato",
    send_failed: "Invio a {name} non riuscito",
    fallback: "{name} non risponde, provo un'altra strada…",
    unreachable_title: "Non riesco a raggiungere «{name}»",
    unreachable_text: "Controlla che Hoppy sia aperto sull'altro dispositivo e che siate sulla stessa rete. Se è un PC Windows: lì clic sull'icona di Hoppy → «Consenti nel firewall…» (su un PC aziendale chiedi all'IT).",
    cancel_send: "Annulla invio",
    close: "Chiudi",
    not_a_file: "«{path}» non è un file: le cartelle arriveranno in una versione futura.",
    no_files: "Nessun file da inviare.",
    too_many_files: "Troppi file selezionati: inviali in più volte.",
    usage: "Uso: hoppyw (icona vicino all'orologio) oppure hoppyw send FILE…",
    update_ready: "Hoppy {version} è disponibile — clic per aggiornare",
    update_portal: "Hoppy {version} è disponibile — clic per scaricarlo",
    menu_update: "Aggiorna a {version}…",
    menu_download: "Scarica Hoppy {version}…",
    update_progress: "Scarico aggiornamento… {pct}%",
    update_failed: "Aggiornamento non riuscito",
    update_mismatch: "Il file scaricato non corrisponde al checksum ufficiale: non è stato installato.",
    fw_title: "Gli altri dispositivi potrebbero non riuscire a inviarti file",
    fw_text: "Clic per consentire Hoppy nel firewall di Windows (serve la conferma di un amministratore).",
    public_title: "La rete è impostata come Pubblica",
    public_text: "Gli altri dispositivi non possono inviarti file: clic per impostarla come Privata.",
    menu_firewall: "Consenti nel firewall…",
    menu_network: "Imposta la rete come Privata…",
    fw_done: "Hoppy è consentito nel firewall ✓",
    fw_failed: "Impossibile consentire Hoppy nel firewall",
    fw_policy: "Le regole sono state aggiunte ma non hanno effetto: probabilmente un criterio aziendale le ignora. Chiedi all'IT di consentire Hoppy (TCP 47471, UDP 47470).",
    menu_classic: "«Invia con Hoppy» direttamente nel tasto destro (menu classico)",
    menu_login: "Apri all'accesso",
    login_failed: "Impossibile cambiare «Apri all'accesso»",
    menu_about: "Informazioni…",
    restart_explorer: "Il nuovo menu del tasto destro compare dopo il riavvio di Esplora risorse (o al prossimo accesso).\n\nRiavviare Esplora risorse ora? Le finestre di Esplora file aperte verranno chiuse.",
    classic_failed: "Impossibile cambiare il menu del tasto destro",
    about_failed: "Impossibile aprire il registro di diagnostica",
    accept: "Accetta",
    decline: "Rifiuta",
    no_dialog_tool: "Per scegliere file e dispositivi Hoppy usa zenity: installalo con «sudo apt install zenity».",
    italian: true,
    sec_code: "🔒 Cifrato · {state}. Per essere sicuro che sia davvero {name}, controlla che sul suo schermo ci siano le stesse 4 immagini:",
    sec_verified: "🔒 Cifrato · ✓ {name} è un dispositivo verificato",
    sec_plain: "⚠ Non cifrato: {name} usa una versione di Hoppy senza cifratura",
    sec_new: "dispositivo nuovo",
    sec_known: "già visto, non verificato",
    match_accept: "Coincide, accetta",
    match_only: "Coincide",
    verify_details: "Verifica…",
    later: "Più tardi",
    code_title: "Hoppy — verifica {name}",
    code_text: "Invio a {name} ({state}) con connessione cifrata 🔒.\nControlla che sull'altro schermo ci siano le stesse 4 immagini, poi premi «Coincide».",
    waiting_code: "In attesa che {name} accetti… codice {code}",
    waiting_plain: "In attesa che {name} accetti… (non cifrato)",
    key_changed_title: "Attenzione: la chiave di sicurezza di «{name}» è cambiata",
    key_changed_text: "Il trasferimento è stato bloccato: {name} si è presentato con una chiave di sicurezza diversa da quella che Hoppy ricorda.\n\nSe {name} è stato reinstallato o ripristinato, scegli «Dimentica dispositivo» e riprova: tornerà a essere un dispositivo nuovo e potrai verificarlo con il codice.\n\nAltrimenti qualcuno sulla rete potrebbe fingersi {name}: chiudi senza dimenticarlo.",
    forget_device: "Dimentica dispositivo",
    forgotten: "«{name}» dimenticato: al prossimo invio sarà un dispositivo nuovo",
    forget_failed: "Impossibile dimenticare «{name}»",
    plain_refused_title: "{name} ha provato a inviare senza cifratura: rifiutato",
    verified_title: "{name} è ora verificato ✓",
    sec_name_clash: "⚠ Un dispositivo che conosci usa già questo nome: potrebbe essere qualcuno che si spaccia per lui",
    aborts_title: "Connessioni interrotte sospette ({n})",
    aborts_text: "Qualcuno su questa rete potrebbe provare a indovinare il codice di verifica: i tentativi vengono rallentati.",
    trust_problem: "Elenco dei dispositivi conosciuti: {error}",
    menu_trusted: "Dispositivi fidati…",
    trusted_header: "Dispositivi ricordati — scegline uno per dimenticarlo:",
    trusted_none: "Nessun dispositivo ricordato",
    trusted_none_text: "Hoppy ricorda un dispositivo quando accetti un suo invio o lo verifichi.",
    trusted_verified: "✓ {name} · {fp}",
    trusted_unverified: "{name} · {fp} · non verificato",
    forget_title: "Dimenticare «{name}»?",
    forget_text: "Al prossimo contatto Hoppy lo tratterà come un dispositivo nuovo e mostrerà di nuovo il codice di verifica.",
    forget: "Dimentica",
    dev_verified: "✓ {label}",
    dev_encrypted: "🔒 {label}",
    dev_plain: "{label} · ⚠ non cifrato",
    update_installing: "Installo Hoppy {version}…",
    update_cancelled: "Aggiornamento annullato",
    update_opened: "Completa l'installazione nel programma di installazione del sistema",
    menu_check_updates: "Controlla aggiornamenti",
    about_version: "Versione {version}",
    about_dev: " · sviluppo",
    about_author: "di {author}",
    about_github: "Pagina GitHub",
    about_diagnostics: "Diagnostica",
    update_current: "Hoppy è aggiornato ({version})",
    update_found: "Hoppy {version} è disponibile — clic su «{item}»",
    update_check_failed: "Non riesco a controllare gli aggiornamenti",
    ui_window_title: "Hoppy — invia file",
    ui_header_one: "Invia «{file}» ({size}) a",
    ui_header_more: "Invia «{file}» +{more} ({size}) a",
    ui_searching: "Cerco dispositivi vicini…",
    ui_searching_text: "Tieni Hoppy aperto sull'altro dispositivo.",
    ui_pick: "Scegli un dispositivo",
    ui_devices: "Dispositivi vicini",
    ui_none_title: "Nessun dispositivo trovato",
    ui_none_text: "Prova così:",
    ui_tip_open: "Apri Hoppy sull'altro dispositivo",
    ui_tip_wifi: "Collegatevi alla stessa rete Wi-Fi",
    ui_tip_firewall: "Su Windows consenti Hoppy nel firewall (icona di Hoppy → «Consenti nel firewall…»)",
    ui_badge_verified: "Verificato",
    ui_badge_encrypted: "Cifrato",
    ui_badge_plain: "Non cifrato",
    ui_send_to: "Invia a {name}, {platform}, {badge}",
    ui_sending: "Invio a {name}…",
    ui_done_text: "La finestra si chiude da sola.",
    ui_declined_text: "Puoi riprovare quando è pronto.",
    ui_retry: "Riprova",
    ui_verify: "Controlla che su {name} ci siano le stesse 4 immagini, poi premi «Coincide».",
    ui_code: "Codice di verifica",
    menu_theme: "Tema",
};

pub const EN: Strings = Strings {
    tip_ready: "Hoppy · {name}",
    menu_header: "Hoppy · receiving as {name}",
    menu_send: "Send files…",
    menu_open: "Open received folder",
    menu_music: "♪ Sound when delivered",
    menu_quit: "Quit",
    pick_title: "Hoppy — choose files to send",
    already_running: "Hoppy is already running: look for its icon near the clock.",
    received_title: "Received {n} file(s) from {name}",
    received_text: "Click to open the folder.",
    receive_failed: "Receiving from {name} failed",
    incoming_title: "Hoppy — incoming files",
    incoming_body: "{name} ({platform}, {ip}) wants to send you {n} file(s), {size}:",
    incoming_more: "… and {n} more",
    searching: "Looking for devices…",
    send_header: "Send “{file}” ({size}) to:",
    send_header_more: "Send “{file}” +{more} ({size}) to:",
    no_devices: "No devices found — open Hoppy on the other device (same Wi-Fi)",
    retry: "Search again",
    cancel: "Cancel",
    waiting: "Waiting for {name} to accept…",
    sending: "Sending to {name}… {pct}%",
    delivered: "Delivered to {name} ✓",
    declined: "{name} declined",
    send_failed: "Sending to {name} failed",
    fallback: "{name} is not answering, trying another way…",
    unreachable_title: "Can't reach \"{name}\"",
    unreachable_text: "Check that Hoppy is open on the other device and that you are on the same network. If it is a Windows PC: click the Hoppy icon there → \"Allow through firewall…\" (on a company PC, ask IT).",
    cancel_send: "Cancel sending",
    close: "Close",
    not_a_file: "\"{path}\" is not a file: folders arrive in a later version.",
    no_files: "Nothing to send.",
    too_many_files: "Too many files selected: send them in several batches.",
    usage: "Usage: hoppyw (icon near the clock) or hoppyw send FILE…",
    update_ready: "Hoppy {version} is available — click to update",
    update_portal: "Hoppy {version} is available — click to download it",
    menu_update: "Update to {version}…",
    menu_download: "Download Hoppy {version}…",
    update_progress: "Downloading update… {pct}%",
    update_failed: "Update failed",
    update_mismatch: "The downloaded file does not match the official checksum: it was not installed.",
    fw_title: "Other devices may not be able to send you files",
    fw_text: "Click to allow Hoppy through Windows Firewall (needs administrator approval).",
    public_title: "This network is set to Public",
    public_text: "Other devices cannot send you files: click to set it to Private.",
    menu_firewall: "Allow through firewall…",
    menu_network: "Set the network to Private…",
    fw_done: "Hoppy is allowed through the firewall ✓",
    fw_failed: "Could not allow Hoppy through the firewall",
    fw_policy: "The rules were added but have no effect: a company policy probably ignores them. Ask IT to allow Hoppy (TCP 47471, UDP 47470).",
    menu_classic: "\"Send with Hoppy\" directly on right-click (classic menu)",
    menu_login: "Open at login",
    login_failed: "Could not change \"Open at login\"",
    menu_about: "About…",
    restart_explorer: "The new right-click menu appears after File Explorer restarts (or at the next sign-in).\n\nRestart File Explorer now? Open File Explorer windows will close.",
    classic_failed: "Could not change the right-click menu",
    about_failed: "Could not open the diagnostic log",
    accept: "Accept",
    decline: "Decline",
    no_dialog_tool: "Hoppy needs zenity to pick files and devices: install it with \"sudo apt install zenity\".",
    italian: false,
    sec_code: "🔒 Encrypted · {state}. To be sure it really is {name}, check that its screen shows the same 4 pictures:",
    sec_verified: "🔒 Encrypted · ✓ {name} is a verified device",
    sec_plain: "⚠ Not encrypted: {name} runs a version of Hoppy without encryption",
    sec_new: "new device",
    sec_known: "seen before, not verified",
    match_accept: "They match, accept",
    match_only: "They match",
    verify_details: "Verify…",
    later: "Later",
    code_title: "Hoppy — verify {name}",
    code_text: "Sending to {name} ({state}) over an encrypted connection 🔒.\nCheck that the other screen shows the same 4 pictures, then press “They match”.",
    waiting_code: "Waiting for {name} to accept… code {code}",
    waiting_plain: "Waiting for {name} to accept… (not encrypted)",
    key_changed_title: "Warning: the security key of “{name}” has changed",
    key_changed_text: "The transfer was blocked: {name} presented a different security key from the one Hoppy remembers.\n\nIf {name} was reinstalled or reset, choose “Forget device” and try again: it becomes a new device and you can verify it with the code.\n\nOtherwise someone on the network may be pretending to be {name}: close without forgetting it.",
    forget_device: "Forget device",
    forgotten: "“{name}” forgotten: it will be a new device next time",
    forget_failed: "Could not forget “{name}”",
    plain_refused_title: "{name} tried to send without encryption: refused",
    verified_title: "{name} is now verified ✓",
    sec_name_clash: "⚠ A device you already know uses this name: someone may be pretending to be it",
    aborts_title: "Suspicious interrupted connections ({n})",
    aborts_text: "Someone on this network may be trying to guess the verification code: their attempts are being slowed down.",
    trust_problem: "List of known devices: {error}",
    menu_trusted: "Trusted devices…",
    trusted_header: "Remembered devices — pick one to forget it:",
    trusted_none: "No remembered devices",
    trusted_none_text: "Hoppy remembers a device when you accept a transfer from it or verify it.",
    trusted_verified: "✓ {name} · {fp}",
    trusted_unverified: "{name} · {fp} · not verified",
    forget_title: "Forget “{name}”?",
    forget_text: "Next time Hoppy treats it as a new device and shows the verification code again.",
    forget: "Forget",
    dev_verified: "✓ {label}",
    dev_encrypted: "🔒 {label}",
    dev_plain: "{label} · ⚠ not encrypted",
    update_installing: "Installing Hoppy {version}…",
    update_cancelled: "Update cancelled",
    update_opened: "Finish the installation in the system's software installer",
    menu_check_updates: "Check for updates",
    about_version: "Version {version}",
    about_dev: " · development",
    about_author: "by {author}",
    about_github: "GitHub page",
    about_diagnostics: "Diagnostics",
    update_current: "Hoppy is up to date ({version})",
    update_found: "Hoppy {version} is available — click “{item}”",
    update_check_failed: "Can't check for updates",
    ui_window_title: "Hoppy — send files",
    ui_header_one: "Send “{file}” ({size}) to",
    ui_header_more: "Send “{file}” +{more} ({size}) to",
    ui_searching: "Looking for nearby devices…",
    ui_searching_text: "Keep Hoppy open on the other device.",
    ui_pick: "Choose a device",
    ui_devices: "Nearby devices",
    ui_none_title: "No devices found",
    ui_none_text: "Try this:",
    ui_tip_open: "Open Hoppy on the other device",
    ui_tip_wifi: "Connect both to the same Wi-Fi network",
    ui_tip_firewall: "On Windows, allow Hoppy through the firewall (Hoppy icon → “Allow through firewall…”)",
    ui_badge_verified: "Verified",
    ui_badge_encrypted: "Encrypted",
    ui_badge_plain: "Not encrypted",
    ui_send_to: "Send to {name}, {platform}, {badge}",
    ui_sending: "Sending to {name}…",
    ui_done_text: "This window closes by itself.",
    ui_declined_text: "You can try again when they are ready.",
    ui_retry: "Try again",
    ui_verify: "Check that {name} shows the same 4 pictures, then press “They match”.",
    ui_code: "Verification code",
    menu_theme: "Theme",
};

pub fn strings() -> &'static Strings {
    if platform::ui_is_italian() { &IT } else { &EN }
}

/// Replaces `{key}` placeholders in one pass (values are never re-scanned, so a device
/// called "{n}" stays literal). Unknown placeholders are kept as-is.
pub fn fill(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let value = tail.find('}').and_then(|end| {
            let key = &tail[1..end];
            args.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v, end))
        });
        match value {
            Some((v, end)) => {
                out.push_str(v);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// "Pixel 8 · Android" (network text cleaned by `display_name`/`display_platform`).
pub fn peer_label(peer: &Peer) -> String {
    let platform = match peer.device.platform.as_str() {
        "android" => "Android".to_owned(),
        "windows" => "Windows".to_owned(),
        "linux" => "Linux".to_owned(),
        "macos" => "macOS".to_owned(),
        "ios" => "iOS".to_owned(),
        other => display_platform(other),
    };
    format!("{} · {platform}", display_name(&peer.device.name))
}

/// Longest file name shown in the send menu's header (characters).
pub const HEADER_FILE_MAX: usize = 40;

/// Header of the device menu: the first file's name (cleaned and cut like network text, so
/// a long or odd name cannot stretch or garble the menu), "+N" for the others, total size.
pub fn send_header(s: &Strings, files: &[PathBuf], total: u64) -> String {
    let first = files
        .first()
        .and_then(|f| f.file_name())
        .map(|n| display_text(&n.to_string_lossy(), HEADER_FILE_MAX))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "?".to_owned());
    let size = human(total);
    match files.len().saturating_sub(1) {
        0 => fill(s.send_header, &[("file", &first), ("size", &size)]),
        more => fill(
            s.send_header_more,
            &[
                ("file", &first),
                ("more", &more.to_string()),
                ("size", &size),
            ],
        ),
    }
}

/// Waits up to `max` for peers, returning early `settle` after the first one appears.
pub fn collect_peers(discovery: &Discovery, max: Duration, settle: Duration) -> Vec<Peer> {
    let start = Instant::now();
    let mut first_seen: Option<Instant> = None;
    loop {
        let peers = discovery.peers();
        let now = Instant::now();
        if !peers.is_empty() && first_seen.is_none() {
            first_seen = Some(now);
        }
        let settled = first_seen.is_some_and(|t| now.duration_since(t) >= settle);
        if settled || now.duration_since(start) >= max {
            return peers;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Body of the Yes/No dialog for an incoming offer. Everything from the network is
/// cleaned first: names via `display_*`, file names as the §4 sanitizer will save them.
pub fn offer_body(s: &Strings, offer: &Offer, from: SocketAddr) -> String {
    let n = offer.files.len().to_string();
    let mut body = fill(
        s.incoming_body,
        &[
            ("name", &display_name(&offer.sender.name)),
            ("platform", &display_platform(&offer.sender.platform)),
            ("ip", &from.ip().to_string()),
            ("n", &n),
            ("size", &human(offer.total_size())),
        ],
    );
    for f in offer.files.iter().take(5) {
        body.push_str("\n• ");
        body.push_str(&display_file_name(&f.name));
    }
    let more = offer.files.len().saturating_sub(5);
    if more > 0 {
        body.push('\n');
        body.push_str(&fill(s.incoming_more, &[("n", &more.to_string())]));
    }
    body
}

/// Buttons of the verification and warning dialogs (`verify::DialogView`).
pub const BTN_DECLINE: u32 = 1;
pub const BTN_ACCEPT: u32 = 2;
/// "They match": accept (receiver) and mark the other device verified.
pub const BTN_MATCH: u32 = 3;
pub const BTN_CLOSE: u32 = 4;
pub const BTN_FORGET: u32 = 5;
/// Buttons of the About dialog.
pub const BTN_CHECK_UPDATES: u32 = 6;
pub const BTN_GITHUB: u32 = 7;
pub const BTN_DIAGNOSTICS: u32 = 8;

/// The project page, shown and opened by the About dialog.
pub const REPO_URL: &str = "https://github.com/Marketto/hoppy";
/// Credited in the About dialog (AGENTS.md invariant 9).
pub const AUTHOR: &str = "Marco Ricupero";

/// The About dialog: name, version (`dev` = a development build, which never checks for
/// updates and so has no such button), author and project link; "Close" is the default.
pub fn about_dialog(s: &Strings, version: &str, dev: bool) -> DialogView {
    let mut version = fill(s.about_version, &[("version", version)]);
    if dev {
        version.push_str(s.about_dev);
    }
    let mut buttons = vec![(BTN_CLOSE, s.close.to_owned())];
    if !dev {
        buttons.push((BTN_CHECK_UPDATES, s.menu_check_updates.to_owned()));
    }
    buttons.extend([
        (BTN_GITHUB, s.about_github.to_owned()),
        (BTN_DIAGNOSTICS, s.about_diagnostics.to_owned()),
    ]);
    DialogView {
        title: "Hoppy".to_owned(),
        text: format!(
            "{version}\n{}\n\n{REPO_URL}",
            fill(s.about_author, &[("author", AUTHOR)])
        ),
        buttons,
        ..Default::default()
    }
}

/// How long the accept question waits (the sender gives up 130 s after its hello).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(110);
/// At most one "suspicious interrupted connections" balloon this often.
const ABORTS_BALLOON_EVERY: Duration = Duration::from_secs(60);

fn state_word(s: &Strings, trust: Option<TrustState>) -> &'static str {
    if trust == Some(TrustState::New) {
        s.sec_new
    } else {
        s.sec_known
    }
}

/// How the session is protected, for the accept question and the code window: encrypted
/// and verified, encrypted with a code to compare, or not encrypted (old app); plus the
/// warning about a new device reusing a known name.
pub fn security_note(s: &Strings, session: &SessionInfo, name: &str) -> String {
    let line = match (session.encrypted, session.trust) {
        (true, Some(TrustState::Verified)) => fill(s.sec_verified, &[("name", name)]),
        (true, trust) => fill(
            s.sec_code,
            &[("state", state_word(s, trust)), ("name", name)],
        ),
        (false, _) => fill(s.sec_plain, &[("name", name)]),
    };
    if session.name_matches_known {
        format!("{line}\n{}", s.sec_name_clash)
    } else {
        line
    }
}

/// The code to compare: only for an encrypted session with a New or Known device.
pub fn code_view(s: &Strings, session: &SessionInfo) -> Option<CodeView> {
    match (session.encrypted, session.trust, &session.sas) {
        (true, Some(TrustState::New | TrustState::Known), Some(sas)) => {
            Some(CodeView::new(sas, s.italian))
        }
        _ => None,
    }
}

/// The receiver's accept question. Decline is the first (default) button; "They match,
/// accept" appears with a code.
pub fn offer_dialog(
    s: &Strings,
    offer: &Offer,
    from: SocketAddr,
    session: &SessionInfo,
) -> DialogView {
    let name = display_name(&offer.sender.name);
    let code = code_view(s, session);
    let mut buttons = vec![
        (BTN_DECLINE, s.decline.to_owned()),
        (BTN_ACCEPT, s.accept.to_owned()),
    ];
    if code.is_some() {
        buttons.push((BTN_MATCH, s.match_accept.to_owned()));
    }
    DialogView {
        title: s.incoming_title.to_owned(),
        text: offer_body(s, offer, from),
        note: security_note(s, session, &name),
        details: code.is_some().then(|| s.verify_details.to_owned()),
        code,
        buttons,
        timeout: Some(ASK_TIMEOUT),
    }
}

/// The sender's code window while the receiver decides (New/Known devices only): the same
/// 4 emoji as on the other screen, "They match" marks the receiver verified here, "Later"
/// (the default) just closes it.
pub fn code_dialog(s: &Strings, name: &str, session: &SessionInfo) -> Option<DialogView> {
    let code = code_view(s, session)?;
    let state = state_word(s, session.trust);
    Some(DialogView {
        title: fill(s.code_title, &[("name", name)]),
        text: fill(s.code_text, &[("name", name), ("state", state)]),
        note: if session.name_matches_known {
            s.sec_name_clash.to_owned()
        } else {
            String::new()
        },
        code: Some(code),
        buttons: vec![
            (BTN_CLOSE, s.later.to_owned()),
            (BTN_MATCH, s.match_only.to_owned()),
        ],
        ..Default::default()
    })
}

/// A remembered device answered with another key (spec §7.5): blocked; "Forget device"
/// is offered but never the default.
pub fn key_changed_dialog(s: &Strings, name: &str) -> DialogView {
    DialogView {
        title: fill(s.key_changed_title, &[("name", name)]),
        text: fill(s.key_changed_text, &[("name", name)]),
        buttons: vec![
            (BTN_CLOSE, s.close.to_owned()),
            (BTN_FORGET, s.forget_device.to_owned()),
        ],
        ..Default::default()
    }
}

/// "Forget X?" from the trusted-devices list.
pub fn forget_dialog(s: &Strings, name: &str) -> DialogView {
    DialogView {
        title: fill(s.forget_title, &[("name", name)]),
        text: s.forget_text.to_owned(),
        buttons: vec![
            (BTN_CLOSE, s.cancel.to_owned()),
            (BTN_FORGET, s.forget.to_owned()),
        ],
        ..Default::default()
    }
}

/// A device in the send menu: ✓ verified, 🔒 encrypted (it announces v1 or its key is
/// remembered), "⚠ not encrypted" for an old app.
pub fn device_label(s: &Strings, peer: &Peer, known: Option<&TrustEntry>) -> String {
    let label = peer_label(peer);
    let template = match known {
        Some(entry) if entry.verified => s.dev_verified,
        Some(_) => s.dev_encrypted,
        None if peer.crypto.is_some_and(|c| c >= 1) => s.dev_encrypted,
        None => s.dev_plain,
    };
    fill(template, &[("label", &label)])
}

/// A remembered device in the "Trusted devices" list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub label: String,
}

/// Remembered devices, by name: "✓ Pixel · ABCD EFGH" (start of the key fingerprint, to
/// tell two devices with the same name apart).
pub fn trusted_devices(s: &Strings, store: &TrustStore) -> Vec<TrustedDevice> {
    let mut list: Vec<TrustedDevice> = store
        .peers
        .iter()
        .map(|(id, entry)| {
            let name = display_name(&entry.name);
            let fp: String = entry.fingerprint.chars().take(8).collect();
            let fp = match fp.char_indices().nth(4) {
                Some((i, _)) => format!("{} {}", &fp[..i], &fp[i..]),
                None => fp,
            };
            let template = if entry.verified {
                s.trusted_verified
            } else {
                s.trusted_unverified
            };
            TrustedDevice {
                id: id.clone(),
                label: fill(template, &[("name", &name), ("fp", &fp)]),
                name,
            }
        })
        .collect();
    list.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    list
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Entry point of `hoppyw`; returns the process exit code.
pub fn run(args: Vec<OsString>) -> i32 {
    let s = strings();
    let mode = args.first().map(|a| a.to_str());
    // Not for `firewall`: the elevated copy may run as another (admin) user.
    // `open` (Linux launcher): with files it sends them, without it is the receiver.
    let mode = match mode {
        Some(Some("open")) if args.len() > 1 => Some(Some("send")),
        Some(Some("open")) => None,
        other => other,
    };
    match mode {
        None => diag::init("tray"),
        Some(Some("send")) => diag::init("send"),
        _ => {}
    }
    diag::log(format!(
        "hoppyw {} started, {} argument(s)",
        update::CURRENT_VERSION,
        args.len()
    ));
    let result = match mode {
        None => tray_mode(s),
        Some(Some("send")) => send_mode(s, args[1..].iter().map(PathBuf::from).collect()),
        Some(Some("firewall")) => return firewall_mode(&args[1..]),
        Some(_) => Err(anyhow::anyhow!("{}", s.usage)),
    };
    let code = match result {
        Ok(()) => 0,
        Err(e) => {
            diag::log(format!("exit with error: {}", diag::error_chain(&e)));
            ui::message_box_error("Hoppy", &format!("{e:#}"));
            1
        }
    };
    diag::log("exit");
    diag::flush(Duration::from_millis(500));
    code
}

/// `hoppyw firewall --apply|--remove` (run elevated by the tray and the installer) and
/// `--check` (installer, see `firewall::check_code`). No UI, since the installer may be
/// silent: the exit code is the result.
fn firewall_mode(args: &[OsString]) -> i32 {
    let exe = std::env::current_exe().ok();
    let Some(dir) = exe.as_deref().and_then(|e| e.parent()) else {
        return 3;
    };
    let result = match args.first().and_then(|a| a.to_str()) {
        Some("--apply") => firewall::apply(dir),
        Some("--remove") => firewall::remove(dir),
        Some("--check") => {
            let status = firewall::check(&firewall::programs_in(dir));
            return firewall::check_code(status.as_ref());
        }
        _ => return 3,
    };
    i32::from(result.is_err())
}

// ---------------------------------------------------------------- tray receiver

struct Notice {
    title: String,
    text: String,
    error: bool,
    /// What clicking this balloon does.
    action: BalloonAction,
}

struct GuiReceiver {
    s: &'static Strings,
    waker: Waker,
    notices: Arc<Mutex<VecDeque<Notice>>>,
    /// This device and its trust store ("They match", "Forget device").
    local: Local,
    /// Sender names by connection, remembered at accept time for the final balloon.
    names: Mutex<HashMap<SocketAddr, String>>,
    /// One accept dialog at a time; offers arriving meanwhile are declined.
    asking: AtomicBool,
    /// Last "suspicious interrupted connections" balloon.
    last_aborts: Mutex<Option<Instant>>,
}

/// One key-change warning dialog at a time (the core already limits them to one a minute
/// per device).
static WARNING_OPEN: AtomicBool = AtomicBool::new(false);
/// One About dialog at a time.
static ABOUT_OPEN: AtomicBool = AtomicBool::new(false);

impl GuiReceiver {
    fn notice(&self, title: String, text: String, error: bool) {
        lock(&self.notices).push_back(Notice {
            title,
            text,
            error,
            action: BalloonAction::Nothing,
        });
        self.waker.wake();
    }
}

impl ReceiveHandler for GuiReceiver {
    fn decide(&self, offer: &Offer, from: SocketAddr, session: &SessionInfo) -> bool {
        let Some(_asking) = DialogGuard::try_take(&self.asking) else {
            diag::log(format!("receive: offer from {} declined (busy)", from.ip()));
            return false; // another offer is waiting for an answer
        };
        let view = offer_dialog(self.s, offer, from, session);
        let answer = ui::ask_offer(&view, &DialogLive::new()).unwrap_or(BTN_DECLINE);
        let accepted = matches!(answer, BTN_ACCEPT | BTN_MATCH);
        diag::log(format!(
            "receive: offer of {} file(s) from {}: accepted={accepted} encrypted={} trust={:?} \
             match={}",
            offer.files.len(),
            from.ip(),
            session.encrypted,
            session.trust,
            answer == BTN_MATCH
        ));
        if answer == BTN_MATCH
            && let Some(peer) = &session.peer
            && let Err(e) = self.local.trust.mark_verified(peer)
        {
            diag::log(format!(
                "trust: marking verified failed: {}",
                diag::error_kind(&e)
            ));
            self.notice(
                fill(
                    self.s.trust_problem,
                    &[("error", &display_text(&e.to_string(), SHOW_TEXT_MAX))],
                ),
                String::new(),
                true,
            );
        }
        if accepted {
            lock(&self.names).insert(from, display_name(&offer.sender.name));
        }
        accepted
    }

    fn finished(&self, from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        let name = lock(&self.names)
            .remove(&from)
            .unwrap_or_else(|| from.ip().to_string());
        let notice = match result {
            Ok(paths) => Notice {
                title: fill(
                    self.s.received_title,
                    &[("n", &paths.len().to_string()), ("name", &name)],
                ),
                text: self.s.received_text.to_owned(),
                error: false,
                action: BalloonAction::OpenDir,
            },
            Err(e) => {
                diag::log(format!(
                    "receive: from {} failed: {}",
                    from.ip(),
                    diag::error_kind(e)
                ));
                Notice {
                    title: fill(self.s.receive_failed, &[("name", &name)]),
                    text: display_text(&e.to_string(), SHOW_TEXT_MAX),
                    error: true,
                    action: BalloonAction::Nothing,
                }
            }
        };
        lock(&self.notices).push_back(notice);
        self.waker.wake();
    }

    fn blocked(&self, from: SocketAddr, offer: &Offer, session: &SessionInfo, reason: &str) {
        let name = display_name(&offer.sender.name);
        diag::log(format!(
            "receive: offer from {} refused: {reason}",
            from.ip()
        ));
        if reason != "key_changed" {
            self.notice(
                fill(self.s.plain_refused_title, &[("name", &name)]),
                String::new(),
                true,
            );
            return;
        }
        // A clear warning with "Forget device", on its own thread: the connection is not
        // kept waiting for the user.
        let id = session
            .peer
            .as_ref()
            .map_or_else(|| offer.sender.id.clone(), |p| p.id.clone());
        let (s, trust, notices, waker) = (
            self.s,
            self.local.trust.clone(),
            Arc::clone(&self.notices),
            self.waker.clone(),
        );
        thread::spawn(move || {
            let Some(_open) = DialogGuard::try_take(&WARNING_OPEN) else {
                diag::log("receive: key-change warning already open");
                return;
            };
            warn_key_changed(s, &trust, &id, &name, &notices, &waker);
        });
    }

    fn warning(&self, from: SocketAddr, warning: &Warning) {
        let (title, text) = match warning {
            Warning::HandshakeAborts { recent } => {
                diag::log(format!(
                    "receive: {recent} aborted handshakes from {}",
                    from.ip()
                ));
                let mut last = lock(&self.last_aborts);
                if last.is_some_and(|t| t.elapsed() < ABORTS_BALLOON_EVERY) {
                    return;
                }
                *last = Some(Instant::now());
                (
                    fill(self.s.aborts_title, &[("n", &recent.to_string())]),
                    self.s.aborts_text.to_owned(),
                )
            }
            Warning::TrustNotSaved(e) => {
                diag::log("trust: the list of known devices could not be saved");
                (
                    fill(
                        self.s.trust_problem,
                        &[("error", &display_text(e, SHOW_TEXT_MAX))],
                    ),
                    String::new(),
                )
            }
        };
        self.notice(title, text, true);
    }
}

/// The blocking key-change warning (worker thread). "Forget device" removes the remembered
/// key so the next contact is New; the result is reported as a balloon.
fn warn_key_changed(
    s: &'static Strings,
    trust: &Trust,
    id: &str,
    name: &str,
    notices: &Mutex<VecDeque<Notice>>,
    waker: &Waker,
) {
    let answer = ui::show_dialog(&key_changed_dialog(s, name), &DialogLive::new());
    if answer != Some(BTN_FORGET) {
        return;
    }
    lock(notices).push_back(forget_device(s, trust, id, name));
    waker.wake();
}

/// Forgets a device (worker: file I/O); the balloon to show about it.
fn forget_device(s: &Strings, trust: &Trust, id: &str, name: &str) -> Notice {
    let (title, error) = match trust.forget(id) {
        Ok(_) => {
            diag::log("trust: a device was forgotten");
            (fill(s.forgotten, &[("name", name)]), false)
        }
        Err(e) => {
            diag::log(format!(
                "trust: forgetting failed: {}",
                diag::error_kind(&e)
            ));
            (fill(s.forget_failed, &[("name", name)]), true)
        }
    };
    Notice {
        title,
        text: String::new(),
        error,
        action: BalloonAction::Nothing,
    }
}

const ID_SEND: u32 = 1;
const ID_OPEN: u32 = 2;
const ID_QUIT: u32 = 3;
const ID_MUSIC: u32 = 4;
const ID_UPDATE: u32 = 5;
const ID_FIREWALL: u32 = 6;
const ID_CLASSIC: u32 = 7;
const ID_ABOUT: u32 = 8;
const ID_LOGIN: u32 = 9;
const ID_TRUSTED: u32 = 10;
/// Entries of the "Theme" submenu: `ID_THEME_FIRST + index` in `Theme::ALL`.
const ID_THEME_FIRST: u32 = 20;
/// Entries of the trusted-devices menu: `ID_TRUSTED_FIRST + index`.
const ID_TRUSTED_FIRST: u32 = 1000;

/// What clicking the last balloon does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BalloonAction {
    Nothing,
    OpenDir,
    Update,
    Portal,
    /// Add the firewall rules (UAC).
    Firewall,
    /// Open Windows network settings (Public → Private).
    Network,
}

/// Settings as last read by a worker. The UI thread only ever reads this copy: device.json
/// lives in the roaming AppData folder, which company PCs often redirect to a network
/// share where a read can take seconds (or minutes when the server is unreachable).
struct Prefs {
    identity: config::Identity,
    /// Windows 11 classic context menu; `None` = not applicable (Windows 10) or unknown.
    classic: Option<bool>,
    refreshing: bool,
    /// A classic-menu change (with its "restart Explorer?" question) is in progress.
    classic_busy: bool,
    /// macOS "Open at login"; `None` = no such switch here.
    login: Option<bool>,
    /// Dark mode / high contrast, for the theme shown as current when none was chosen.
    look: SystemLook,
}

/// Serialises the workers' device.json reads/writes (and their cache updates). Never taken
/// by the UI thread.
static CONFIG_IO: Mutex<()> = Mutex::new(());

/// What tray workers need to report back to the UI thread.
#[derive(Clone)]
struct Ctx {
    s: &'static Strings,
    waker: Waker,
    prefs: Arc<Mutex<Prefs>>,
    notices: Arc<Mutex<VecDeque<Notice>>>,
    trust: Trust,
    /// The trusted-devices list, read by a worker, waiting to be shown by the UI thread.
    trusted: Arc<Mutex<Option<Vec<TrustedDevice>>>>,
}

impl Ctx {
    /// Error balloon (shown by the UI thread on the next `Wake`), also logged.
    fn notify_error(&self, title: &str, text: &str) {
        diag::log(format!("error: {title}: {text}"));
        lock(&self.notices).push_back(Notice {
            title: title.to_owned(),
            text: display_text(text, SHOW_TEXT_MAX),
            error: true,
            action: BalloonAction::Nothing,
        });
        self.waker.wake();
    }

    /// Re-reads device.json and the classic-menu switch in the background (each menu open,
    /// so `hoppy music on|off` is picked up). At most one refresh at a time.
    fn refresh_prefs(&self) {
        {
            let mut p = lock(&self.prefs);
            if p.refreshing {
                return;
            }
            p.refreshing = true;
        }
        let ctx = self.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let classic = platform::classic_menu();
            let login = platform::login_item();
            let look = platform::system_look();
            let io = lock(&CONFIG_IO);
            let identity = config::load_or_create();
            let mut p = lock(&ctx.prefs);
            match identity {
                Ok(identity) => p.identity = identity,
                Err(e) => diag::log(format!("settings: reading device.json failed: {e:#}")),
            }
            if !p.classic_busy {
                p.classic = classic; // else `set_classic` reports the new state itself
            }
            p.login = login;
            p.look = look;
            p.refreshing = false;
            drop((p, io));
            if !ui::NATIVE_POPUPS {
                ctx.waker.wake(); // rebuild the persistent tray menu
            }
            let took = started.elapsed();
            if took >= diag::SLOW_HANDLER {
                diag::log(format!("settings: refresh took {} ms", took.as_millis()));
            }
        });
    }

    /// Runs a device.json change on a worker; its result becomes the cached identity.
    fn config_job(
        &self,
        what: &'static str,
        job: impl FnOnce() -> Result<config::Identity> + Send + 'static,
    ) {
        let ctx = self.clone();
        thread::spawn(move || {
            let io = lock(&CONFIG_IO);
            match job() {
                Ok(identity) => lock(&ctx.prefs).identity = identity,
                Err(e) => {
                    drop(io);
                    ctx.notify_error("Hoppy", &format!("{what}: {e:#}"));
                }
            }
        });
    }

    fn set_music(&self, on: bool) {
        lock(&self.prefs).identity.music = on;
        self.config_job("music", move || config::set_music(on));
    }

    /// Tray "Theme" submenu: the send window's look from now on (device.json `theme`).
    fn set_theme(&self, theme: Theme) {
        lock(&self.prefs).identity.theme = theme.id().to_owned();
        diag::log(format!("theme: {}", theme.id()));
        self.config_job("theme", move || config::set_theme(theme.id()));
    }

    /// Turns the classic context menu on/off, then offers to restart Explorer (worker:
    /// registry writes, a Yes/No dialog and taskkill must not block the tray).
    fn set_classic(&self, on: bool) {
        {
            let mut p = lock(&self.prefs);
            if p.classic_busy {
                return;
            }
            p.classic_busy = true;
            p.classic = Some(on);
        }
        let ctx = self.clone();
        thread::spawn(move || {
            let result = platform::set_classic_menu(on);
            lock(&ctx.prefs).classic = platform::classic_menu();
            match result {
                Ok(true) => {
                    if platform::ask_accept("Hoppy", ctx.s.restart_explorer)
                        && let Err(e) = platform::restart_explorer()
                    {
                        ctx.notify_error(ctx.s.classic_failed, &format!("{e:#}"));
                    }
                }
                Ok(false) => {}
                Err(e) => ctx.notify_error(ctx.s.classic_failed, &format!("{e:#}")),
            }
            lock(&ctx.prefs).classic_busy = false;
        });
    }

    /// Turns "Open at login" on/off (worker: launchd/ServiceManagement calls), then
    /// re-reads the real state for the menu.
    fn set_login(&self, on: bool) {
        lock(&self.prefs).login = Some(on);
        let ctx = self.clone();
        thread::spawn(move || {
            let result = platform::set_login_item(on);
            lock(&ctx.prefs).login = platform::login_item();
            if let Err(e) = result {
                ctx.notify_error(ctx.s.login_failed, &format!("{e:#}"));
            }
        });
    }

    fn notify(&self, notice: Notice) {
        lock(&self.notices).push_back(notice);
        self.waker.wake();
    }

    /// "Trusted devices…": reads the list on a worker (trust.json), then the UI thread
    /// shows it (`show_trusted`).
    fn load_trusted(&self) {
        let ctx = self.clone();
        thread::spawn(move || {
            let list = trusted_devices(ctx.s, &ctx.trust.snapshot());
            *lock(&ctx.trusted) = Some(list);
            ctx.waker.wake();
        });
    }

    /// "Forget X?" and the forgetting itself, on a worker.
    fn confirm_forget(&self, device: TrustedDevice) {
        let ctx = self.clone();
        thread::spawn(move || {
            let answer = ui::show_dialog(&forget_dialog(ctx.s, &device.name), &DialogLive::new());
            if answer == Some(BTN_FORGET) {
                ctx.notify(forget_device(ctx.s, &ctx.trust, &device.id, &device.name));
            }
        });
    }

    /// "About…": the About dialog on a worker (one at a time); its buttons check for
    /// updates (answered by a balloon), open the project page or open the log.
    fn show_about(&self, mode: UpdateMode, updates: &Arc<Mutex<Updates>>) {
        let (ctx, updates) = (self.clone(), Arc::clone(updates));
        thread::spawn(move || {
            let Some(_open) = DialogGuard::try_take(&ABOUT_OPEN) else {
                return;
            };
            let view = about_dialog(ctx.s, update::CURRENT_VERSION, mode == UpdateMode::Off);
            match ui::show_dialog(&view, &DialogLive::new()) {
                Some(BTN_CHECK_UPDATES) if mode != UpdateMode::Off => {
                    request_update_check(&updates, &ctx.waker, update::Trigger::Manual);
                }
                Some(BTN_GITHUB) => {
                    if !ui::open_url(REPO_URL) {
                        diag::log("about: opening the project page failed");
                    }
                }
                Some(BTN_DIAGNOSTICS) => ctx.open_log(mode),
                _ => {}
            }
        });
    }

    /// Writes a summary line, then opens the log in Notepad (TextEdit on macOS, the default
    /// editor on Linux).
    fn open_log(&self, mode: UpdateMode) {
        let (music, classic) = {
            let p = lock(&self.prefs);
            (p.identity.music, p.classic)
        };
        diag::log(format!(
            "about: Hoppy {}, updates={}, music={music}, classic_menu={classic:?}",
            update::CURRENT_VERSION,
            match mode {
                UpdateMode::Off => "off",
                UpdateMode::Setup => "setup",
                UpdateMode::Portal => "portable",
            }
        ));
        let ctx = self.clone();
        thread::spawn(move || {
            diag::flush(Duration::from_secs(2));
            let opened = match diag::log_path() {
                Some(path) => log_viewer()
                    .arg(path)
                    .spawn()
                    .map(|_| ())
                    .map_err(anyhow::Error::from),
                None => Err(anyhow::anyhow!("LOCALAPPDATA is not set")),
            };
            if let Err(e) = opened {
                ctx.notify_error(ctx.s.about_failed, &format!("{e:#}"));
            }
        });
    }
}

/// The program that shows the diagnostic log (the path is appended).
fn log_viewer() -> Command {
    if cfg!(windows) {
        Command::new("notepad.exe")
    } else if cfg!(target_os = "macos") {
        let mut open = Command::new("/usr/bin/open");
        open.arg("-t"); // the default text editor
        open
    } else {
        Command::new("xdg-open")
    }
}

/// Opens a folder/URL on a worker (ShellExecute can block on a slow network folder).
fn open_dir(dir: &std::path::Path) {
    let dir = dir.to_path_buf();
    thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        if !ui::open_folder(&dir) {
            diag::log("opening the received folder failed");
        }
    });
}

fn open_url(url: &'static str) {
    thread::spawn(move || {
        if !ui::open_url(url) {
            diag::log(format!("opening {url} failed"));
        }
    });
}

/// Short name of an event for the UI timing log.
fn event_name(event: &TrayEvent) -> &'static str {
    match event {
        TrayEvent::Click(_) => "click",
        TrayEvent::BalloonClick => "balloon click",
        TrayEvent::Wake => "wake",
        TrayEvent::Reopen => "reopen",
        TrayEvent::Timer(_) => "timer",
        TrayEvent::Menu(_) => "menu",
        TrayEvent::MenuOpening => "menu opening",
    }
}

/// What the tray menu shows besides the fixed entries.
struct TrayMenu {
    /// (clickable, text) of the update entry.
    update: Option<(bool, String)>,
    firewall: Option<&'static str>,
    music: bool,
    classic: Option<bool>,
    login: Option<bool>,
    /// The "Theme" submenu, the theme in use checked.
    themes: Vec<MenuItem<'static>>,
}

/// The "Theme" submenu entries (`ID_THEME_FIRST + index`), `current` checked.
fn theme_items(current: Theme) -> Vec<MenuItem<'static>> {
    Theme::ALL
        .iter()
        .zip(ID_THEME_FIRST..)
        .map(|(&t, id)| MenuItem::Check(id, t.label(), t == current))
        .collect()
}

/// The theme of a "Theme" submenu id.
fn theme_of_id(id: u32) -> Option<Theme> {
    let index = usize::try_from(id.checked_sub(ID_THEME_FIRST)?).ok()?;
    Theme::ALL.get(index).copied()
}

impl TrayMenu {
    fn items<'a>(&'a self, s: &'a Strings, header: &'a str) -> Vec<MenuItem<'a>> {
        let mut items = Vec::new();
        if let Some((clickable, text)) = &self.update {
            items.push(if *clickable {
                MenuItem::Bold(ID_UPDATE, text)
            } else {
                MenuItem::Label(text)
            });
            items.push(MenuItem::Separator);
        }
        items.extend([
            MenuItem::Label(header),
            MenuItem::Action(ID_SEND, s.menu_send),
            MenuItem::Action(ID_OPEN, s.menu_open),
            MenuItem::Check(ID_MUSIC, s.menu_music, self.music),
            MenuItem::Sub(s.menu_theme, &self.themes),
        ]);
        if let Some(on) = self.classic {
            items.push(MenuItem::Check(ID_CLASSIC, s.menu_classic, on));
        }
        if let Some(on) = self.login {
            items.push(MenuItem::Check(ID_LOGIN, s.menu_login, on));
        }
        if let Some(text) = self.firewall {
            items.push(MenuItem::Action(ID_FIREWALL, text));
        }
        items.extend([
            MenuItem::Separator,
            MenuItem::Action(ID_TRUSTED, s.menu_trusted),
            MenuItem::Action(ID_ABOUT, s.menu_about),
            MenuItem::Action(ID_QUIT, s.menu_quit),
        ]);
        items
    }
}

fn tray_mode(s: &'static Strings) -> Result<()> {
    let Some(_instance) = ui::single_instance(INSTANCE_MUTEX) else {
        diag::log("tray: already running, asking it to show itself");
        ui::signal_existing(TRAY_WINDOW);
        return Ok(());
    };
    // Before the icon exists, so nothing is waiting on the UI thread yet.
    let identity = config::load_or_create()?;
    let local = config::local(&identity)?;
    let trust_warning = local.trust.take_warning();
    if let Some(w) = &trust_warning {
        diag::log(format!("trust: {w}"));
    }
    let me = local.device.clone();
    let dir = default_receive_dir()?;
    let listener = bind_receiver(TRANSFER_PORT)?;
    let discovery = start_discovery(&me, listener.local_addr()?.port(), &[])?;

    let tray = Tray::new(TRAY_WINDOW, &fill(s.tip_ready, &[("name", &me.name)]))?;
    let notices = Arc::new(Mutex::new(VecDeque::new()));
    let ctx = Ctx {
        s,
        waker: tray.waker(),
        prefs: Arc::new(Mutex::new(Prefs {
            identity,
            classic: None,
            refreshing: false,
            classic_busy: false,
            login: None,
            look: SystemLook::default(),
        })),
        notices: Arc::clone(&notices),
        trust: local.trust.clone(),
        trusted: Arc::new(Mutex::new(None)),
    };
    ctx.refresh_prefs(); // classic-menu state
    if let Some(w) = trust_warning {
        // e.g. a damaged trust.json was set aside: every device looks new again.
        ctx.notify_error(
            &fill(
                s.trust_problem,
                &[("error", &display_text(&w, SHOW_TEXT_MAX))],
            ),
            "",
        );
    }
    let handler = Arc::new(GuiReceiver {
        s,
        waker: tray.waker(),
        notices: Arc::clone(&notices),
        local: local.clone(),
        names: Mutex::new(HashMap::new()),
        asking: AtomicBool::new(false),
        last_aborts: Mutex::new(None),
    });
    // Same folder and handler for both paths, so `GuiReceiver::asking` covers callbacks too.
    accept_callbacks(&discovery, dir.clone(), local.clone(), handler.clone());
    let serve_dir = dir.clone();
    thread::spawn(move || serve(listener, serve_dir, local, handler));

    let updates = Arc::new(Mutex::new(Updates::default()));
    let mode = update_mode();
    // Installers of earlier updates (their folder could not be removed while they ran), and
    // what an interrupted Mac update left next to the app.
    thread::spawn(|| {
        update::remove_stale_update_dirs(&std::env::temp_dir(), update::STALE_UPDATE_DIR);
        platform::remove_update_leftovers();
    });
    if mode != UpdateMode::Off {
        spawn_update_checker(&updates, tray.waker());
    }
    let fw = Arc::new(Mutex::new(FirewallState::default()));
    spawn_firewall_check(&fw, tray.waker());

    // Linux: right-click entries, launcher and autostart; macOS: services refresh and
    // "Open at login"; once per version (release builds only: a dev build must not point
    // the user's desktop at target/).
    if !cfg!(windows) && mode != UpdateMode::Off {
        ctx.config_job("desktop integration", platform::first_run_integration);
    }

    let ready_tip = fill(s.tip_ready, &[("name", &me.name)]);
    let header = fill(s.menu_header, &[("name", &me.name)]);
    // The tray menu as of now; on Windows built at each click, on Linux kept up to date
    // after every event (`Tray::set_menu`).
    let menu = || {
        let (music, classic, login, current) = {
            let p = lock(&ctx.prefs);
            let current = theme::effective(&p.identity.theme, p.look);
            (p.identity.music, p.classic, p.login, current)
        };
        TrayMenu {
            update: update_menu_text(s, &lock(&updates), mode),
            firewall: firewall_menu_text(s, &fw, &tray),
            music,
            classic,
            login,
            themes: theme_items(current),
        }
    };
    // Runs a menu pick; false = quit. `shown`: the menu the user picked from (toggles flip
    // what they saw, not a value refreshed meanwhile).
    let on_pick = |id: u32, shown: &TrayMenu, h: &mut diag::Handler<'_>| -> bool {
        match id {
            ID_UPDATE => start_update(s, &updates, mode, tray.waker()),
            ID_SEND => match h.modal(|| tray.pick_files(s.pick_title)) {
                Ok(files) if files.is_empty() => {}
                Ok(files) => spawn_sender(&ctx, files),
                Err(TooManyFiles) => {
                    h.modal(|| ui::message_box_error("Hoppy", s.too_many_files));
                }
            },
            ID_OPEN => open_dir(&dir),
            ID_FIREWALL => match lock(&fw).verdict {
                Some(Verdict::PublicNetwork) => open_url(NETWORK_SETTINGS),
                _ => start_firewall_fix(&ctx, &fw),
            },
            ID_MUSIC => ctx.set_music(!shown.music),
            ID_CLASSIC => ctx.set_classic(!shown.classic.unwrap_or(false)),
            ID_LOGIN => ctx.set_login(!shown.login.unwrap_or(false)),
            ID_TRUSTED => ctx.load_trusted(),
            ID_ABOUT => ctx.show_about(mode, &updates),
            ID_QUIT => return false,
            id => {
                if let Some(theme) = theme_of_id(id) {
                    ctx.set_theme(theme);
                }
            }
        }
        true
    };
    let mut balloon_action = BalloonAction::Nothing;
    let watch = diag::UiWatch::start();
    // Linux: the menu currently published (`Tray::set_menu`).
    let mut published = menu();
    if !ui::NATIVE_POPUPS {
        tray.set_menu(&published.items(s, &header));
    }
    while let Some(event) = tray.next_event() {
        let mut h = watch.handler(event_name(&event));
        match event {
            TrayEvent::Click(anchor) => {
                let current = menu();
                // For the next menu: `hoppy music on|off` may have changed it meanwhile,
                // and a new release may be out (throttled; shown at the next click).
                ctx.refresh_prefs();
                if mode != UpdateMode::Off {
                    request_update_check(&updates, &tray.waker(), update::Trigger::MenuOpened);
                }
                let items = current.items(s, &header);
                let picked = h.modal(|| tray.popup_menu(&items, true, anchor));
                if let Some(id) = picked
                    && !on_pick(id, &current, &mut h)
                {
                    break;
                }
            }
            TrayEvent::Menu(id) => {
                if !on_pick(id, &published, &mut h) {
                    break;
                }
            }
            TrayEvent::MenuOpening => {
                ctx.refresh_prefs();
                // Linux: a release found now is published by the `Wake` that follows.
                if mode != UpdateMode::Off {
                    request_update_check(&updates, &tray.waker(), update::Trigger::MenuOpened);
                }
            }
            TrayEvent::Wake => {
                loop {
                    // Pop first so the lock is not held during the UI call.
                    let next = lock(&notices).pop_front();
                    let Some(n) = next else { break };
                    // System sound on: this is the "files arrived" notification.
                    tray.balloon(&n.title, &n.text, n.error, true);
                    balloon_action = n.action;
                }
                if let Some(action) = firewall_wake(&tray, &ctx, &fw) {
                    balloon_action = action;
                }
                let trusted = lock(&ctx.trusted).take();
                match trusted {
                    Some(list) if list.is_empty() => {
                        tray.balloon(s.trusted_none, s.trusted_none_text, false, true);
                        balloon_action = BalloonAction::Nothing;
                    }
                    Some(list) => {
                        if let Some(device) = h.modal(|| pick_trusted(&tray, s, &list)) {
                            ctx.confirm_forget(device);
                        }
                    }
                    None => {}
                }
                let upd = update_wake(&tray, s, &updates, mode, &ready_tip);
                if let Some(action) = upd.balloon {
                    balloon_action = action;
                }
                if upd.restart {
                    match platform::relaunch_after_exit() {
                        Ok(()) => {
                            diag::log("update: installed, restarting");
                            break;
                        }
                        Err(e) => {
                            diag::log(format!("update: restart failed: {e:#}"));
                            let text = display_text(&format!("{e:#}"), SHOW_TEXT_MAX);
                            tray.set_tip(&ready_tip);
                            tray.balloon(s.update_failed, &text, true, true);
                            balloon_action = BalloonAction::Nothing;
                        }
                    }
                }
                if let Some(path) = upd.launch {
                    diag::log("update: starting the verified installer and exiting");
                    match Command::new(&path).args(update::INSTALLER_ARGS).spawn() {
                        // The installer stops running copies anyway; leave cleanly first
                        // (dropping `tray` removes the icon) and let it restart us.
                        Ok(_) => break,
                        Err(e) => {
                            diag::log(format!("update: starting the installer failed: {e}"));
                            let text = display_text(&e.to_string(), SHOW_TEXT_MAX);
                            tray.set_tip(&ready_tip);
                            tray.balloon(s.update_failed, &text, true, true);
                            balloon_action = BalloonAction::Nothing;
                        }
                    }
                }
            }
            TrayEvent::BalloonClick => match balloon_action {
                BalloonAction::OpenDir => open_dir(&dir),
                BalloonAction::Update => start_update(s, &updates, mode, tray.waker()),
                BalloonAction::Portal => open_url(update::PORTAL_URL),
                BalloonAction::Firewall => start_firewall_fix(&ctx, &fw),
                BalloonAction::Network => open_url(NETWORK_SETTINGS),
                BalloonAction::Nothing => {}
            },
            TrayEvent::Reopen => tray.balloon("Hoppy", s.already_running, false, true),
            _ => {}
        }
        if !ui::NATIVE_POPUPS {
            published = menu();
            tray.set_menu(&published.items(s, &header));
        }
    }
    // Locals drop in reverse order: the tray icon goes first, then discovery says "bye".
    Ok(())
}

/// The trusted-devices menu (a list dialog on Linux): the device picked to forget.
fn pick_trusted(tray: &Tray, s: &Strings, list: &[TrustedDevice]) -> Option<TrustedDevice> {
    let mut items = vec![MenuItem::Label(s.trusted_header), MenuItem::Separator];
    for (i, device) in list.iter().enumerate() {
        let id = ID_TRUSTED_FIRST.checked_add(u32::try_from(i).ok()?)?;
        items.push(MenuItem::Action(id, &device.label));
    }
    let picked = tray.popup_menu(&items, false, None)?;
    let index = picked.checked_sub(ID_TRUSTED_FIRST)?;
    list.get(usize::try_from(index).ok()?).cloned()
}

// ---------------------------------------------------------------- firewall

/// Windows Settings page with the Public/Private switch of the current network.
const NETWORK_SETTINGS: &str = "ms-settings:network-status";
/// Re-check (in the background) when the menu opens at most this often, so the menu item
/// goes away after the user fixed things elsewhere.
const FIREWALL_RECHECK: Duration = Duration::from_secs(60);

#[derive(Default)]
struct FirewallState {
    /// Latest result; `None` until the first check ends, or when it failed (no hint then).
    verdict: Option<Verdict>,
    /// A check ended and the UI thread has not looked at it yet.
    fresh: bool,
    checking: bool,
    checked_at: Option<Instant>,
    /// The UAC prompt / elevated `hoppyw firewall --apply` is running.
    fixing: bool,
    /// Its result (exit code 0), for the UI thread to report.
    fixed: Option<bool>,
}

/// Blocking (PowerShell, seconds): workers only. The lock is never held around it.
fn check_now() -> Option<Verdict> {
    let exe = std::env::current_exe().ok()?;
    let started = Instant::now();
    let verdict = firewall::check(&[exe]).map(|s| s.verdict(0));
    diag::log(format!(
        "firewall: check {verdict:?} in {} ms",
        started.elapsed().as_millis()
    ));
    verdict
}

fn spawn_firewall_check(fw: &Arc<Mutex<FirewallState>>, waker: Waker) {
    {
        let mut f = lock(fw);
        if f.checking || f.fixing {
            return;
        }
        f.checking = true;
    }
    let fw = Arc::clone(fw);
    thread::spawn(move || {
        let verdict = check_now();
        let mut f = lock(&fw);
        f.verdict = verdict;
        f.fresh = true;
        f.checking = false;
        f.checked_at = Some(Instant::now());
        drop(f);
        waker.wake();
    });
}

/// Menu item text while there is something to fix; starts a background re-check when the
/// last one is old.
fn firewall_menu_text(
    s: &'static Strings,
    fw: &Arc<Mutex<FirewallState>>,
    tray: &Tray,
) -> Option<&'static str> {
    let (verdict, stale) = {
        let f = lock(fw);
        let stale = f
            .checked_at
            .is_some_and(|t| t.elapsed() >= FIREWALL_RECHECK);
        (if f.fixing { None } else { f.verdict }, stale)
    };
    if stale {
        spawn_firewall_check(fw, tray.waker());
    }
    match verdict? {
        Verdict::NeedsRules => Some(s.menu_firewall),
        Verdict::PublicNetwork => Some(s.menu_network),
        Verdict::Ok => None,
    }
}

/// UAC prompt for `hoppyw firewall --apply` on a worker (the prompt blocks until answered);
/// the result is reported by `firewall_wake`. Declining the prompt is silent.
fn start_firewall_fix(ctx: &Ctx, fw: &Arc<Mutex<FirewallState>>) {
    {
        let mut f = lock(fw);
        if f.fixing {
            return;
        }
        f.fixing = true;
    }
    let (ctx, fw) = (ctx.clone(), Arc::clone(fw));
    thread::spawn(move || {
        let child = std::env::current_exe()
            .map_err(anyhow::Error::from)
            .and_then(|exe| ui::run_elevated(Some(&ctx.waker), &exe, "firewall --apply"));
        let child = match child {
            Ok(child) => child,
            Err(e) => {
                lock(&fw).fixing = false;
                if e.is::<ElevationDeclined>() {
                    diag::log("firewall: UAC declined");
                } else {
                    ctx.notify_error(ctx.s.fw_failed, &format!("{e:#}"));
                }
                return;
            }
        };
        let ok = child.wait(120_000) == Some(0);
        diag::log(format!("firewall: apply ok={ok}"));
        let verdict = check_now();
        let mut f = lock(&fw);
        f.fixing = false;
        f.fixed = Some(ok);
        f.verdict = verdict;
        f.checked_at = Some(Instant::now());
        drop(f);
        ctx.waker.wake();
    });
}

/// UI-thread side of `Wake` for the firewall: the hint (once per version) or the result of
/// a fix. Returns the new balloon action when a balloon was shown.
fn firewall_wake(tray: &Tray, ctx: &Ctx, fw: &Mutex<FirewallState>) -> Option<BalloonAction> {
    let s = ctx.s;
    let (fresh, fixed, verdict) = {
        let mut f = lock(fw);
        (std::mem::take(&mut f.fresh), f.fixed.take(), f.verdict)
    };
    if let Some(ok) = fixed {
        let action = match (ok, verdict) {
            (false, _) => {
                tray.balloon(s.fw_failed, "", true, true);
                BalloonAction::Nothing
            }
            (true, Some(Verdict::NeedsRules)) => {
                tray.balloon(s.fw_failed, s.fw_policy, true, true);
                BalloonAction::Nothing
            }
            (true, Some(Verdict::PublicNetwork)) => {
                tray.balloon(s.public_title, s.public_text, false, true);
                BalloonAction::Network
            }
            (true, _) => {
                tray.balloon(s.fw_done, "", false, true);
                BalloonAction::Nothing
            }
        };
        return Some(action);
    }
    if !fresh {
        return None;
    }
    let (title, text, action) = match verdict? {
        Verdict::NeedsRules => (s.fw_title, s.fw_text, BalloonAction::Firewall),
        Verdict::PublicNetwork => (s.public_title, s.public_text, BalloonAction::Network),
        Verdict::Ok => return None,
    };
    {
        let mut p = lock(&ctx.prefs);
        if p.identity.firewall_hint == update::CURRENT_VERSION {
            return None;
        }
        p.identity.firewall_hint = update::CURRENT_VERSION.to_owned();
    }
    ctx.config_job("firewall hint", || {
        config::set_firewall_hint(update::CURRENT_VERSION)
    });
    tray.balloon(title, text, false, true);
    Some(action)
}

// ---------------------------------------------------------------- updates

#[derive(Clone, Copy, PartialEq, Eq)]
enum UpdateMode {
    /// Dev build: never check.
    Off,
    /// This copy installs updates itself (`platform::can_self_update`): Windows setup
    /// install, /Applications/Hoppy.app, the .deb. Download, verify, install.
    Setup,
    /// Portable or hand-built copy: only point to the download portal.
    Portal,
}

fn update_mode() -> UpdateMode {
    if cfg!(debug_assertions) || update::is_dev_version(update::CURRENT_VERSION) {
        return UpdateMode::Off;
    }
    if platform::can_self_update() {
        UpdateMode::Setup
    } else {
        UpdateMode::Portal
    }
}

/// Shared between the update threads and the UI thread.
#[derive(Default)]
struct Updates {
    available: Option<update::Release>,
    /// `available` changed and has not been announced with a balloon yet.
    fresh: bool,
    /// Some(percent) while the package is downloading.
    downloading: Option<u64>,
    /// macOS/Linux: the verified package is being installed (`Some(announced)`).
    installing: Option<bool>,
    /// The worker's result, or a message to show.
    finished: Option<Result<update::UpdateReady, String>>,
    /// A check is running.
    checking: bool,
    /// When the last check started (wall clock; failed checks count too).
    last_check: Option<SystemTime>,
    /// "Check for updates" was picked: the running/next check answers with a balloon.
    manual: bool,
    /// That answer, waiting for the UI thread.
    answer: Option<CheckAnswer>,
}

/// The answer to "Check for updates".
#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckAnswer {
    UpToDate,
    /// This version can be installed with the menu entry.
    Available(String),
    /// The error, cleaned for display.
    Failed(String),
}

impl Updates {
    /// Takes the right to check now for `trigger`: false when one is running or it is too
    /// soon (`update::check_due`). A manual request waiting on a running check is answered
    /// by that check.
    fn claim_check(&mut self, trigger: update::Trigger, now: SystemTime) -> bool {
        if trigger == update::Trigger::Manual {
            self.manual = true;
        }
        if self.checking || !update::check_due(trigger, self.last_check, now) {
            return false;
        }
        self.checking = true;
        self.last_check = Some(now);
        true
    }

    /// Records the result of a check; true when the UI thread has something to show.
    fn finish_check(&mut self, result: Result<Option<update::Release>, String>) -> bool {
        self.checking = false;
        let mut news = false;
        if let Ok(Some(release)) = &result
            && self.available.as_ref() != Some(release)
            && self.downloading.is_none()
            && self.installing.is_none()
        {
            self.available = Some(release.clone());
            self.fresh = true;
            news = true;
        }
        if std::mem::take(&mut self.manual) {
            self.answer = Some(match result {
                Err(e) => CheckAnswer::Failed(e),
                Ok(_) => match &self.available {
                    // The answer says it; no second "is available" balloon.
                    Some(release) => {
                        self.fresh = false;
                        CheckAnswer::Available(release.version.to_string())
                    }
                    None => CheckAnswer::UpToDate,
                },
            });
            news = true;
        }
        news
    }
}

/// WinHTTP on Windows, curl/wget on Linux.
struct NativeHttp;

impl update::Http for NativeHttp {
    fn get(&self, url: &str, sink: &mut HttpSink<'_>) -> Result<()> {
        ui::http_get(url, update::USER_AGENT, sink)
    }
}

/// Checks after `FIRST_CHECK`, then every `CHECK_EVERY` of wall-clock time and once after
/// a wake-up from sleep (`update::Cadence`: one-minute ticks, so a laptop that sleeps a
/// lot still checks on time). Menu opens and "Check for updates" add their own checks
/// (`request_update_check`). Network errors are only logged.
fn spawn_update_checker(updates: &Arc<Mutex<Updates>>, waker: Waker) {
    let updates = Arc::clone(updates);
    thread::spawn(move || {
        thread::sleep(update::FIRST_CHECK);
        let mut cadence = update::Cadence::default();
        loop {
            let last = lock(&updates).last_check;
            if let Some(trigger) = cadence.tick(SystemTime::now(), last) {
                run_update_check(&updates, &waker, trigger);
            }
            thread::sleep(update::CHECK_TICK);
        }
    });
}

/// A check for `trigger` on a worker, if it is due (UI thread: never blocks).
fn request_update_check(updates: &Arc<Mutex<Updates>>, waker: &Waker, trigger: update::Trigger) {
    if !lock(updates).claim_check(trigger, SystemTime::now()) {
        return;
    }
    let (updates, waker) = (Arc::clone(updates), waker.clone());
    thread::spawn(move || check_claimed(&updates, &waker, trigger));
}

/// Claims and runs a check on this (worker) thread.
fn run_update_check(updates: &Mutex<Updates>, waker: &Waker, trigger: update::Trigger) {
    if lock(updates).claim_check(trigger, SystemTime::now()) {
        check_claimed(updates, waker, trigger);
    }
}

fn check_claimed(updates: &Mutex<Updates>, waker: &Waker, trigger: update::Trigger) {
    let result = update::check(&NativeHttp, update::CURRENT_VERSION);
    match &result {
        Ok(Some(release)) => diag::log(format!(
            "update: {} is available ({trigger:?})",
            release.version
        )),
        Ok(None) => diag::log(format!("update: up to date ({trigger:?})")),
        // Offline, proxy…: silent unless the user asked, but kept for diagnosis.
        Err(e) => diag::log(format!("update: check failed ({trigger:?}): {e:#}")),
    }
    let result = result.map_err(|e| display_text(&format!("{e:#}"), SHOW_TEXT_MAX));
    if lock(updates).finish_check(result) {
        waker.wake();
    }
}

/// The balloon answering "Check for updates": (title, text, error, click action).
fn check_answer_balloon(
    s: &Strings,
    answer: &CheckAnswer,
    mode: UpdateMode,
) -> (String, String, bool, BalloonAction) {
    match answer {
        CheckAnswer::UpToDate => (
            fill(s.update_current, &[("version", update::CURRENT_VERSION)]),
            String::new(),
            false,
            BalloonAction::Nothing,
        ),
        CheckAnswer::Available(version) => {
            let (item, action) = match mode {
                UpdateMode::Setup => (s.menu_update, BalloonAction::Update),
                _ => (s.menu_download, BalloonAction::Portal),
            };
            let item = fill(item, &[("version", version)]);
            (
                fill(s.update_found, &[("version", version), ("item", &item)]),
                String::new(),
                false,
                action,
            )
        }
        CheckAnswer::Failed(error) => (
            s.update_check_failed.to_owned(),
            error.clone(),
            true,
            BalloonAction::Nothing,
        ),
    }
}

/// Top menu entry: (clickable, text). Bold "Update to X…", or the progress while
/// downloading.
fn update_menu_text(s: &Strings, u: &Updates, mode: UpdateMode) -> Option<(bool, String)> {
    if u.installing.is_some() {
        let version = u.available.as_ref().map(|r| r.version.to_string());
        let version = version.unwrap_or_default();
        return Some((false, fill(s.update_installing, &[("version", &version)])));
    }
    if let Some(pct) = u.downloading {
        return Some((false, fill(s.update_progress, &[("pct", &pct.to_string())])));
    }
    let version = u.available.as_ref()?.version.to_string();
    let template = match mode {
        UpdateMode::Setup => s.menu_update,
        UpdateMode::Portal => s.menu_download,
        UpdateMode::Off => return None,
    };
    Some((true, fill(template, &[("version", &version)])))
}

/// Starts the download and install (Setup) or opens the portal (portable). One at a time.
fn start_update(
    s: &'static Strings,
    updates: &Arc<Mutex<Updates>>,
    mode: UpdateMode,
    waker: Waker,
) {
    if mode == UpdateMode::Portal {
        open_url(update::PORTAL_URL);
        return;
    }
    let release = {
        let mut u = lock(updates);
        if mode != UpdateMode::Setup || u.downloading.is_some() || u.installing.is_some() {
            return;
        }
        let Some(release) = u.available.clone() else {
            return;
        };
        u.downloading = Some(0);
        release
    };
    waker.wake(); // show 0% now
    let updates = Arc::clone(updates);
    thread::spawn(move || {
        let progress = &mut |pct| {
            let mut u = lock(&updates);
            if u.downloading != Some(pct) {
                u.downloading = Some(pct);
                drop(u);
                waker.wake(); // at most ~100 wake-ups
            }
        };
        let installing = &mut || {
            diag::log("update: verified, installing");
            let mut u = lock(&updates);
            u.downloading = None;
            u.installing = Some(false);
            drop(u);
            waker.wake();
        };
        let result = platform::fetch_update(&NativeHttp, &release, progress, installing);
        if let Ok(ready) = &result {
            diag::log(format!("update: {ready:?}"));
        }
        let result = result.map_err(|e| {
            diag::log(format!("update: failed: {e:#}"));
            if e.is::<update::HashMismatch>() {
                s.update_mismatch.to_owned()
            } else {
                display_text(&format!("{e:#}"), SHOW_TEXT_MAX)
            }
        });
        lock(&updates).finished = Some(result);
        waker.wake();
    });
}

#[derive(Default)]
struct UpdateWake {
    /// New action for clicks on the balloon just shown.
    balloon: Option<BalloonAction>,
    /// Verified installer to run now (Windows).
    launch: Option<PathBuf>,
    /// The new version is installed: start it and exit (macOS, Linux).
    restart: bool,
}

/// UI-thread side of `Wake` for updates: announce, show progress, report the result.
fn update_wake(
    tray: &Tray,
    s: &Strings,
    updates: &Mutex<Updates>,
    mode: UpdateMode,
    ready_tip: &str,
) -> UpdateWake {
    let (announce, pct, installing, finished, answer) = {
        let mut u = lock(updates);
        let answer = u.answer.take();
        let version = u.available.as_ref().map(|r| r.version.to_string());
        let announce = std::mem::take(&mut u.fresh)
            .then(|| version.clone())
            .flatten();
        // Announce the install step once.
        let installing = (u.installing == Some(false)).then(|| version.unwrap_or_default());
        if installing.is_some() {
            u.installing = Some(true);
        }
        let finished = u.finished.take();
        if finished.is_some() {
            u.downloading = None;
            u.installing = None;
        }
        (announce, u.downloading, installing, finished, answer)
    };
    let mut out = UpdateWake::default();
    if let Some(answer) = answer {
        let (title, text, error, action) = check_answer_balloon(s, &answer, mode);
        tray.balloon(&title, &text, error, true);
        out.balloon = Some(action);
    }
    if let Some(version) = announce {
        let (template, action) = match mode {
            UpdateMode::Setup => (s.update_ready, BalloonAction::Update),
            _ => (s.update_portal, BalloonAction::Portal),
        };
        tray.balloon(&fill(template, &[("version", &version)]), "", false, true);
        out.balloon = Some(action);
    }
    if let Some(version) = installing {
        let text = fill(s.update_installing, &[("version", &version)]);
        tray.set_tip(&text);
        tray.balloon(&text, "", false, false);
        out.balloon = Some(BalloonAction::Nothing);
    }
    match finished {
        Some(Ok(update::UpdateReady::RunInstaller(path))) => out.launch = Some(path),
        Some(Ok(update::UpdateReady::Restart)) => out.restart = true,
        Some(Ok(update::UpdateReady::Cancelled)) => {
            tray.set_tip(ready_tip);
            tray.balloon(s.update_cancelled, "", false, false);
            out.balloon = Some(BalloonAction::Nothing);
        }
        Some(Ok(update::UpdateReady::OpenedInstaller)) => {
            tray.set_tip(ready_tip);
            tray.balloon(s.update_opened, "", false, true);
            out.balloon = Some(BalloonAction::Nothing);
        }
        Some(Err(text)) => {
            tray.set_tip(ready_tip);
            tray.balloon(s.update_failed, &text, true, true);
            // Clicking it opens the portal: the manual way still works.
            out.balloon = Some(BalloonAction::Portal);
        }
        None => {
            if let Some(pct) = pct {
                tray.set_tip(&fill(s.update_progress, &[("pct", &pct.to_string())]));
            }
        }
    }
    out
}

/// Runs `hoppyw send FILE…` as a separate process (one tray icon per transfer), from a
/// worker: process creation can stall while antivirus software scans the exe.
fn spawn_sender(ctx: &Ctx, files: Vec<PathBuf>) {
    let ctx = ctx.clone();
    thread::spawn(move || {
        let child = std::env::current_exe()
            .and_then(|exe| Command::new(exe).arg("send").args(&files).spawn());
        match child {
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(e) => ctx.notify_error("Hoppy", &e.to_string()),
        }
    });
}

// ---------------------------------------------------------------- "Send to → Hoppy"

/// The sender's only sound: the done jingle on delivery. Stops when dropped (before the
/// process exits).
struct Jingle {
    enabled: bool,
    playing: bool,
}

impl Jingle {
    fn new(enabled: bool) -> Jingle {
        Jingle {
            enabled,
            playing: false,
        }
    }

    /// Plays the jingle unless muted. Returns whether it plays.
    fn play(&mut self) -> bool {
        if self.enabled {
            ui::play_done_jingle();
            self.playing = true;
        }
        self.enabled
    }
}

impl Drop for Jingle {
    fn drop(&mut self) {
        if self.playing {
            ui::stop_sound();
        }
    }
}

/// A device found by the search, with its menu label and window card (✓ / 🔒 / ⚠, read
/// from the trust store).
struct Found {
    peer: Peer,
    label: String,
    card: DeviceCard,
}

#[derive(Default)]
struct SendState {
    /// Found devices.
    peers: Option<Vec<Found>>,
    /// The receiver said yes (first `Progress::FileStarted`).
    accepted: bool,
    pct: Option<u64>,
    /// The direct connection failed; waiting for a callback (spec §6).
    fallback: bool,
    /// The session is set up (spec §7): what to show while waiting for "Accept".
    session: Option<SessionInfo>,
    outcome: Option<Result<SendOutcome, SendError>>,
    /// Balloons from workers (code window "They match", "Forget device").
    notices: VecDeque<Notice>,
    /// The key-change warning was closed: the process may exit now.
    warning_closed: bool,
    /// The open code window; the send worker closes it with the outcome (on macOS the UI
    /// loop waits while that window is up, so it cannot do it itself).
    code_window: Option<DialogLive>,
    /// Send window: "They match" / "Forget device" finished (ok, message).
    verify_result: Option<(bool, String)>,
    forget_result: Option<(bool, String)>,
}

/// Workers report to whichever UI drives the send: the tray loop (`ui::Waker`) or the
/// send window (`sendui::window::Waker`).
trait Wake: Clone + Send + 'static {
    fn wake(&self);
}

impl Wake for Waker {
    fn wake(&self) {
        Waker::wake(self);
    }
}

impl Wake for sendui::window::Waker {
    fn wake(&self) {
        sendui::window::Waker::wake(self);
    }
}

enum SendError {
    /// Neither direct nor callback reached the peer: show the firewall hint.
    Unreachable,
    /// The peer's key is not the remembered one: nothing was sent (spec §7.5).
    KeyChanged,
    Other(String),
}

enum Phase {
    Searching,
    Sending(String),
    Done,
}

const ID_RETRY: u32 = 1;
const ID_CANCEL: u32 = 2;
const ID_PEER: u32 = 100;

fn send_mode(s: &'static Strings, files: Vec<PathBuf>) -> Result<()> {
    let files = if files.is_empty() {
        // Desktop shortcut double-clicked (dropping files on it passes them as arguments).
        match ui::pick_files_unowned(s.pick_title) {
            Ok(files) if files.is_empty() => return Ok(()),
            Ok(files) => files,
            Err(TooManyFiles) => bail!("{}", s.too_many_files),
        }
    } else {
        // "Send with Hoppy" on several files: one process per file, merged here.
        match platform::gather_send_files(files) {
            Ok(Some(files)) => files,
            Ok(None) => return Ok(()), // handed to the first instance
            Err(TooManyFiles) => bail!("{}", s.too_many_files),
        }
    };
    if files.is_empty() {
        bail!("{}", s.no_files);
    }
    if let Some(f) = files.iter().find(|f| !f.is_file()) {
        bail!(
            "{}",
            fill(s.not_a_file, &[("path", &f.display().to_string())])
        );
    }
    let total: u64 = files
        .iter()
        .filter_map(|f| f.metadata().ok())
        .map(|m| m.len())
        .sum();
    let identity = config::load_or_create()?;
    let local = config::local(&identity)?;
    let discovery = Arc::new(start_discovery(&local.device, 0, &[])?);
    diag::log(format!("send: {} file(s), {}", files.len(), human(total)));
    // The OS look only matters when no theme was chosen (it runs gsettings/defaults).
    let look = if Theme::from_id(&identity.theme).is_some() {
        SystemLook::default()
    } else {
        platform::system_look()
    };
    let theme = theme::effective(&identity.theme, look);
    match send_window(s, &files, total, &identity, &local, &discovery, theme) {
        Ok(()) => Ok(()),
        Err(e) => {
            diag::log(format!("send: window unavailable ({e}); native picker"));
            send_native(s, files, total, &identity, &local, &discovery)
        }
    }
}

/// The themed send window (`sendui`, ADR 0011): searching, device cards, progress, the
/// verification panel and the results in one window. `Err` only when the window could
/// not start (nothing was shown yet): the caller then uses the native picker.
fn send_window(
    s: &'static Strings,
    files: &[PathBuf],
    total: u64,
    identity: &config::Identity,
    local: &Local,
    discovery: &Arc<Discovery>,
    theme: Theme,
) -> Result<(), sendui::window::Unavailable> {
    use sendui::window::{self, Input};
    use sendui::{Effect, Flow, Outcome};

    let first = files
        .first()
        .and_then(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let header = sendui::header(s, &first, files.len(), &human(total));
    let state = Arc::new(Mutex::new(SendState::default()));
    let mut flow = Flow::new();
    // The devices of the last search (the page only knows their index).
    let mut found: Vec<Peer> = Vec::new();
    let mut chosen: Option<Peer> = None;
    let mut session_peer: Option<PeerKey> = None;
    let mut jingle = Jingle::new(identity.music);
    let init = sendui::Init::new(s, theme);
    window::run(s.ui_window_title, theme, &init, |ctl, input| {
        let mut effects = Vec::new();
        match input {
            Input::Started => {
                spawn_search(s, discovery, &local.trust, &state, ctl.waker());
            }
            Input::Ready => diag::log("send: window ready"),
            Input::Action(action) => effects = flow.action(action),
            Input::Wake => {
                let mut st = lock(&state);
                if let Some(list) = st.peers.take() {
                    diag::log(format!("send: {} device(s) found", list.len()));
                    let (peers, cards): (Vec<Peer>, Vec<DeviceCard>) =
                        list.into_iter().map(|f| (f.peer, f.card)).unzip();
                    found = peers;
                    effects.extend(flow.found(cards));
                }
                if let Some(session) = st.session.take() {
                    session_peer = session.peer.clone();
                    flow.session(session, s.italian);
                }
                flow.progress(st.accepted, st.pct, st.fallback);
                if let Some((ok, message)) = st.verify_result.take() {
                    flow.verify_result(ok, message);
                }
                if let Some((ok, message)) = st.forget_result.take() {
                    flow.forget_result(ok, message);
                }
                if let Some(outcome) = st.outcome.take() {
                    if outcome.is_ok() {
                        diag::log(format!("send: done (callback: {})", st.fallback));
                    }
                    let outcome = match outcome {
                        Ok(SendOutcome::Delivered) => Outcome::Delivered,
                        // "declined" = the user said No; other reasons are protocol limits.
                        Ok(SendOutcome::Declined(reason)) => Outcome::Declined(
                            reason
                                .filter(|r| r != "declined")
                                .map(|r| display_text(&r, SHOW_TEXT_MAX)),
                        ),
                        Err(SendError::Unreachable) => Outcome::Unreachable,
                        Err(SendError::KeyChanged) => Outcome::KeyChanged,
                        Err(SendError::Other(e)) => {
                            Outcome::Failed(display_text(&e, SHOW_TEXT_MAX))
                        }
                    };
                    effects.extend(flow.outcome(outcome));
                }
            }
        }
        for effect in effects {
            match effect {
                Effect::Search => {
                    discovery.refresh();
                    *lock(&state) = SendState::default();
                    spawn_search(s, discovery, &local.trust, &state, ctl.waker());
                }
                Effect::Send(i) => {
                    let Some(peer) = found.get(i).cloned() else {
                        continue;
                    };
                    chosen = Some(peer.clone());
                    session_peer = None;
                    *lock(&state) = SendState::default();
                    spawn_send(discovery, local, peer, files.to_vec(), &state, ctl.waker());
                }
                Effect::MarkVerified => {
                    if let Some(peer) = session_peer.clone() {
                        let (trust, state, waker) =
                            (local.trust.clone(), Arc::clone(&state), ctl.waker());
                        thread::spawn(move || {
                            let n = mark_verified(s, &trust, &peer);
                            lock(&state).verify_result = Some((!n.error, n.title));
                            waker.wake();
                        });
                    }
                }
                Effect::Forget => {
                    if let Some(peer) = &chosen {
                        let (id, name) = (peer.device.id.clone(), display_name(&peer.device.name));
                        let (trust, state, waker) =
                            (local.trust.clone(), Arc::clone(&state), ctl.waker());
                        thread::spawn(move || {
                            let n = forget_device(s, &trust, &id, &name);
                            lock(&state).forget_result = Some((!n.error, n.title));
                            waker.wake();
                        });
                    }
                }
                Effect::Jingle => {
                    jingle.play();
                }
                Effect::OnTop(on) => ctl.set_on_top(on),
                Effect::CloseAfter(delay) => ctl.close_after(delay),
                Effect::Exit => ctl.exit(),
            }
        }
        ctl.render(&flow.view(s, &header));
    })
}

/// The native send UI (the fallback when the window cannot start): progress in the tray
/// icon, a popup menu (Windows), a panel (macOS) or a zenity list (Linux) of devices.
fn send_native(
    s: &'static Strings,
    files: Vec<PathBuf>,
    total: u64,
    identity: &config::Identity,
    local: &Local,
    discovery: &Arc<Discovery>,
) -> Result<()> {
    let tray = Tray::new(SEND_WINDOW, s.searching)?;
    // Linux: a Cancel button on the "Looking for devices…" notification.
    tray.set_menu(&[MenuItem::Action(ID_CANCEL, s.cancel)]);
    // Declared after `tray`, so it is dropped (jingle stopped) before the icon goes.
    let mut jingle = Jingle::new(identity.music);
    let state = Arc::new(Mutex::new(SendState::default()));
    spawn_search(s, discovery, &local.trust, &state, tray.waker());
    // A code window was shown (at most one per send).
    let mut code_shown = false;
    // The device chosen (its id for "Forget device").
    let mut chosen: Option<Peer> = None;

    let header = send_header(s, &files, total);
    let mut phase = Phase::Searching;
    let mut files = Some(files);
    let watch = diag::UiWatch::start();
    while let Some(event) = tray.next_event() {
        let mut h = watch.handler(event_name(&event));
        match (event, &phase) {
            (TrayEvent::Wake, Phase::Searching) => {
                let Some(found) = lock(&state).peers.take() else {
                    continue;
                };
                let (peers, labels): (Vec<Peer>, Vec<String>) =
                    found.into_iter().map(|f| (f.peer, f.label)).unzip();
                let mut items = vec![MenuItem::Label(&header), MenuItem::Separator];
                if peers.is_empty() {
                    items.push(MenuItem::Label(s.no_devices));
                }
                for (i, label) in labels.iter().enumerate() {
                    items.push(MenuItem::Action(ID_PEER + i as u32, label));
                }
                items.extend([MenuItem::Separator, MenuItem::Action(ID_RETRY, s.retry)]);
                if ui::NATIVE_POPUPS {
                    items.push(MenuItem::Action(ID_CANCEL, s.cancel)); // dialogs have their own
                }
                diag::log(format!("send: {} device(s) found", peers.len()));
                match h.modal(|| tray.popup_menu(&items, false, None)) {
                    Some(ID_RETRY) => {
                        tray.set_tip(s.searching);
                        discovery.refresh();
                        spawn_search(s, discovery, &local.trust, &state, tray.waker());
                    }
                    Some(id) if id >= ID_PEER => {
                        let Some(peer) = peers.get((id - ID_PEER) as usize).cloned() else {
                            break;
                        };
                        let name = display_name(&peer.device.name);
                        // Linux: a "Cancel sending" button on the progress notification.
                        tray.set_menu(&[MenuItem::Action(ID_CANCEL, s.cancel_send)]);
                        tray.set_tip(&fill(s.waiting, &[("name", &name)]));
                        let files = files.take().unwrap_or_default();
                        chosen = Some(peer.clone());
                        spawn_send(discovery, local, peer, files, &state, tray.waker());
                        phase = Phase::Sending(name);
                    }
                    _ => break, // "Cancel" or menu dismissed
                }
            }
            (TrayEvent::Wake, Phase::Sending(name)) => {
                let (accepted, pct, fallback, session, outcome, notices) = {
                    let mut st = lock(&state);
                    let session = st.session.clone();
                    let notices = std::mem::take(&mut st.notices);
                    (
                        st.accepted,
                        st.pct,
                        st.fallback,
                        session,
                        st.outcome.take(),
                        notices,
                    )
                };
                for n in notices {
                    tray.balloon(&n.title, &n.text, n.error, false);
                }
                if let Some(outcome) = outcome {
                    if let Some(window) = lock(&state).code_window.take() {
                        window.close();
                    }
                    let key_changed = matches!(outcome, Err(SendError::KeyChanged));
                    let delivered = matches!(outcome, Ok(SendOutcome::Delivered));
                    // Our jingle is the sound of success; skip the system one on top of it.
                    let sound = !(delivered && jingle.play());
                    // Failures were logged by kind in `spawn_send` (texts may hold names).
                    if outcome.is_ok() {
                        diag::log(format!("send: done (callback: {fallback})"));
                    }
                    let (title, text, error) = match outcome {
                        Ok(SendOutcome::Delivered) => (s.delivered, String::new(), false),
                        Ok(SendOutcome::Declined(reason)) => {
                            // "declined" = the user said No; other reasons are protocol limits.
                            let reason = reason.filter(|r| r != "declined").unwrap_or_default();
                            (s.declined, display_text(&reason, SHOW_TEXT_MAX), true)
                        }
                        Err(SendError::Unreachable) => {
                            (s.unreachable_title, s.unreachable_text.to_owned(), true)
                        }
                        Err(SendError::KeyChanged) => (
                            s.key_changed_title,
                            fill(s.key_changed_text, &[("name", name)]),
                            true,
                        ),
                        Err(SendError::Other(e)) => {
                            (s.send_failed, display_text(&e, SHOW_TEXT_MAX), true)
                        }
                    };
                    let title = fill(title, &[("name", name)]);
                    tray.set_tip(&title);
                    if key_changed {
                        // A blocking warning with "Forget device"; exit once it is closed.
                        let peer_id = chosen.as_ref().map(|p| p.device.id.clone());
                        spawn_key_warning(s, local, peer_id, name, &state, tray.waker());
                    } else {
                        tray.balloon(&title, &text, error, sound);
                        tray.set_timer(EXIT_TIMER, EXIT_AFTER_MS);
                    }
                    phase = Phase::Done;
                } else if let Some(pct) = pct {
                    let pct = pct.to_string();
                    tray.set_tip(&fill(s.sending, &[("name", name), ("pct", &pct)]));
                } else if let (Some(session), false) = (&session, accepted) {
                    let tip = match &session.sas {
                        Some(sas) => fill(s.waiting_code, &[("name", name), ("code", &sas.text)]),
                        None => fill(s.waiting_plain, &[("name", name)]),
                    };
                    tray.set_tip(&tip);
                    // New/Known receiver: the same code in a window, with "They match".
                    if !code_shown
                        && let Some(view) = code_dialog(s, name, session)
                        && let Some(peer) = session.peer.clone()
                    {
                        code_shown = true;
                        let live = DialogLive::new();
                        let mut st = lock(&state);
                        if st.outcome.is_none() {
                            st.code_window = Some(live.clone());
                            drop(st);
                            let trust = local.trust.clone();
                            spawn_code_window(view, live, trust, peer, &state, tray.waker());
                        }
                    }
                } else if fallback && !accepted {
                    tray.set_tip(&fill(s.fallback, &[("name", name)]));
                }
            }
            (TrayEvent::Click(anchor), Phase::Sending(_)) => {
                let items = [MenuItem::Action(ID_CANCEL, s.cancel_send)];
                if h.modal(|| tray.popup_menu(&items, true, anchor)) == Some(ID_CANCEL) {
                    break; // closing the process aborts the transfer
                }
            }
            (TrayEvent::Wake, Phase::Done) => {
                let (notices, closed) = {
                    let mut st = lock(&state);
                    (
                        std::mem::take(&mut st.notices),
                        std::mem::take(&mut st.warning_closed),
                    )
                };
                for n in notices {
                    tray.balloon(&n.title, &n.text, n.error, false);
                }
                if closed {
                    tray.set_timer(EXIT_TIMER, EXIT_AFTER_MS);
                }
            }
            (TrayEvent::Click(anchor), Phase::Done) => {
                let items = [MenuItem::Action(ID_CANCEL, s.close)];
                if h.modal(|| tray.popup_menu(&items, true, anchor)) == Some(ID_CANCEL) {
                    break;
                }
            }
            (TrayEvent::Menu(ID_CANCEL), _) => break,
            (TrayEvent::Timer(EXIT_TIMER), _) => {
                tray.kill_timer(EXIT_TIMER);
                break;
            }
            _ => {}
        }
    }
    Ok(()) // dropping `tray` removes the icon
}

/// Searches on a worker; the labels say which devices are verified/encrypted (reading the
/// trust store is file I/O, kept off the UI thread).
fn spawn_search(
    s: &'static Strings,
    discovery: &Arc<Discovery>,
    trust: &Trust,
    state: &Arc<Mutex<SendState>>,
    waker: impl Wake,
) {
    let (discovery, state, trust) = (Arc::clone(discovery), Arc::clone(state), trust.clone());
    thread::spawn(move || {
        let peers = collect_peers(&discovery, SEARCH_MAX, SEARCH_SETTLE);
        let store = trust.snapshot();
        let labelled = peers
            .into_iter()
            .map(|peer| {
                let known = store.peers.get(&peer.device.id);
                Found {
                    label: device_label(s, &peer, known),
                    card: sendui::device_card(s, &peer, known),
                    peer,
                }
            })
            .collect();
        lock(&state).peers = Some(labelled);
        waker.wake();
    });
}

/// The sender's code window on a worker (a custom window on Windows, an alert on the Mac's
/// main thread, zenity on Linux). "They match" marks the receiver verified here.
fn spawn_code_window(
    view: DialogView,
    live: DialogLive,
    trust: Trust,
    peer: PeerKey,
    state: &Arc<Mutex<SendState>>,
    waker: Waker,
) {
    let state = Arc::clone(state);
    let s = strings();
    thread::spawn(move || {
        if ui::show_dialog(&view, &live) != Some(BTN_MATCH) {
            return;
        }
        let notice = mark_verified(s, &trust, &peer);
        lock(&state).notices.push_back(notice);
        waker.wake();
    });
}

/// "They match" on the sender: marks the receiver verified (worker: file I/O); the message
/// to show about it.
fn mark_verified(s: &Strings, trust: &Trust, peer: &PeerKey) -> Notice {
    let name = display_name(&peer.name);
    match trust.mark_verified(peer) {
        Ok(()) => {
            diag::log("send: receiver marked verified");
            Notice {
                title: fill(s.verified_title, &[("name", &name)]),
                text: String::new(),
                error: false,
                action: BalloonAction::Nothing,
            }
        }
        Err(e) => {
            diag::log(format!(
                "send: marking verified failed: {}",
                diag::error_kind(&e)
            ));
            Notice {
                title: fill(
                    s.trust_problem,
                    &[("error", &display_text(&e.to_string(), SHOW_TEXT_MAX))],
                ),
                text: String::new(),
                error: true,
                action: BalloonAction::Nothing,
            }
        }
    }
}

/// `hoppyw send` hit a changed key: the warning dialog with "Forget device" on a worker;
/// the process exits once it is closed (`warning_closed`).
fn spawn_key_warning(
    s: &'static Strings,
    local: &Local,
    peer_id: Option<String>,
    name: &str,
    state: &Arc<Mutex<SendState>>,
    waker: Waker,
) {
    let (trust, state, name) = (local.trust.clone(), Arc::clone(state), name.to_owned());
    thread::spawn(move || {
        let answer = ui::show_dialog(&key_changed_dialog(s, &name), &DialogLive::new());
        let mut st = lock(&state);
        if answer == Some(BTN_FORGET)
            && let Some(id) = peer_id
        {
            drop(st);
            let notice = forget_device(s, &trust, &id, &name);
            st = lock(&state);
            st.notices.push_back(notice);
        }
        st.warning_closed = true;
        drop(st);
        waker.wake();
    });
}

fn spawn_send(
    discovery: &Arc<Discovery>,
    local: &Local,
    peer: Peer,
    files: Vec<PathBuf>,
    state: &Arc<Mutex<SendState>>,
    waker: impl Wake,
) {
    let (discovery, state, local) = (Arc::clone(discovery), Arc::clone(state), local.clone());
    thread::spawn(move || {
        let mut last = None;
        let result = send_to_peer(&peer, &local, Some(&discovery), &files, &mut |p| match p {
            Progress::Session(session) => {
                diag::log(format!("send: encrypted={}", session.encrypted));
                lock(&state).session = Some(session);
                waker.wake();
            }
            Progress::Warning(w) => diag::log(format!("send: warning {w:?}")),
            // Direct connect failed (firewalled receiver?): asking it to connect back.
            Progress::Fallback => {
                diag::log("send: direct connection failed, trying a callback");
                lock(&state).fallback = true;
                waker.wake();
            }
            // Only sent after the receiver accepted (ends the "trying another way" tip).
            Progress::FileStarted { index: 0, .. } => {
                lock(&state).accepted = true;
                waker.wake();
            }
            Progress::Bytes { done, total } => {
                let pct = done.saturating_mul(100).checked_div(total).unwrap_or(100);
                if last != Some(pct) {
                    last = Some(pct);
                    lock(&state).pct = Some(pct);
                    waker.wake(); // at most ~100 wake-ups per transfer
                }
            }
            _ => {}
        });
        if let Err(e) = &result {
            diag::log(format!("send: failed: {}", diag::error_kind(e)));
        }
        let mut st = lock(&state);
        if let Some(window) = st.code_window.take() {
            window.close();
        }
        st.outcome = Some(result.map_err(|e| match e {
            Error::Unreachable(_) => SendError::Unreachable,
            Error::KeyChanged(_) => SendError::KeyChanged,
            e => SendError::Other(e.to_string()),
        }));
        drop(st);
        waker.wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoppy_core::{DeviceInfo, DiscoveryConfig};

    fn placeholders(t: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = t
            .split('{')
            .skip(1)
            .filter_map(|p| p.split_once('}').map(|(k, _)| k))
            .collect();
        keys.sort_unstable();
        keys
    }

    #[test]
    fn italian_and_english_use_the_same_placeholders() {
        let pairs = |s: &Strings| {
            [
                s.tip_ready,
                s.menu_header,
                s.received_title,
                s.receive_failed,
                s.incoming_body,
                s.incoming_more,
                s.send_header,
                s.send_header_more,
                s.waiting,
                s.sending,
                s.delivered,
                s.declined,
                s.send_failed,
                s.fallback,
                s.unreachable_title,
                s.unreachable_text,
                s.not_a_file,
                s.update_ready,
                s.update_portal,
                s.menu_update,
                s.menu_download,
                s.update_progress,
                s.sec_code,
                s.sec_verified,
                s.sec_plain,
                s.code_title,
                s.code_text,
                s.waiting_code,
                s.waiting_plain,
                s.key_changed_title,
                s.key_changed_text,
                s.forgotten,
                s.forget_failed,
                s.plain_refused_title,
                s.verified_title,
                s.aborts_title,
                s.trust_problem,
                s.trusted_verified,
                s.trusted_unverified,
                s.forget_title,
                s.dev_verified,
                s.dev_encrypted,
                s.dev_plain,
                s.update_installing,
                s.update_current,
                s.update_found,
                s.about_version,
                s.about_author,
                s.ui_header_one,
                s.ui_header_more,
                s.ui_send_to,
                s.ui_sending,
                s.ui_verify,
            ]
        };
        for (it, en) in pairs(&IT).iter().zip(pairs(&EN)) {
            assert_eq!(placeholders(it), placeholders(en), "{it} / {en}");
        }
    }

    fn release(tag: &str) -> update::Release {
        let json = serde_json::json!({
            "tag_name": tag,
            "assets": [{ "name": update::PLATFORM_ASSET }, { "name": update::SUMS_ASSET }],
        });
        update::parse_latest_for(json.to_string().as_bytes(), update::PLATFORM_ASSET).unwrap()
    }

    #[test]
    fn update_checks_are_throttled_and_answered() {
        use update::Trigger::{Manual, MenuOpened, Resumed, Scheduled};
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let min = |m: u64| t0 + Duration::from_secs(m * 60);
        let mut u = Updates::default();
        assert!(u.claim_check(Scheduled, t0));
        // One at a time.
        assert!(!u.claim_check(MenuOpened, t0));
        assert!(!u.manual);
        assert!(!u.finish_check(Ok(None))); // up to date, nobody asked: nothing to show
        assert_eq!(u.last_check, Some(t0));
        // Menu opens and wake-ups: at most every 10 minutes.
        assert!(!u.claim_check(MenuOpened, min(5)));
        assert!(!u.claim_check(Resumed, min(9)));
        assert!(u.claim_check(MenuOpened, min(10)));
        // A new release: announced once (fresh), not again by the next check.
        assert!(u.finish_check(Ok(Some(release("v9.0.0")))));
        assert!(u.fresh && u.answer.is_none());
        u.fresh = false;
        assert!(u.claim_check(MenuOpened, min(20)));
        assert!(!u.finish_check(Ok(Some(release("v9.0.0")))));
        assert!(!u.fresh);

        // "Check for updates" always checks and always answers.
        let mut u = Updates::default();
        assert!(u.claim_check(Manual, t0));
        assert!(u.finish_check(Ok(None)));
        assert_eq!(u.answer.take(), Some(CheckAnswer::UpToDate));
        assert!(u.claim_check(Manual, t0));
        assert!(u.finish_check(Err("offline".into())));
        assert_eq!(u.answer.take(), Some(CheckAnswer::Failed("offline".into())));
        // Found by a manual check: one balloon (the answer), not the "available" one too.
        assert!(u.claim_check(Manual, t0));
        assert!(u.finish_check(Ok(Some(release("v9.1.0")))));
        assert!(!u.fresh);
        assert_eq!(
            u.answer.take(),
            Some(CheckAnswer::Available("9.1.0".into()))
        );
        // Asked while an automatic check runs: that check answers.
        assert!(u.claim_check(Scheduled, min(500)));
        assert!(!u.claim_check(Manual, min(500)));
        assert!(u.finish_check(Ok(None)));
        // Still offering the release found before.
        assert_eq!(
            u.answer.take(),
            Some(CheckAnswer::Available("9.1.0".into()))
        );
        assert!(!u.manual);
    }

    #[test]
    fn check_answer_balloons() {
        let (title, text, error, action) =
            check_answer_balloon(&IT, &CheckAnswer::UpToDate, UpdateMode::Setup);
        assert_eq!(
            title,
            format!("Hoppy è aggiornato ({})", update::CURRENT_VERSION)
        );
        assert!(text.is_empty() && !error && action == BalloonAction::Nothing);
        let found = CheckAnswer::Available("0.8.0".into());
        let (title, _, error, action) = check_answer_balloon(&IT, &found, UpdateMode::Setup);
        assert_eq!(
            title,
            "Hoppy 0.8.0 è disponibile — clic su «Aggiorna a 0.8.0…»"
        );
        assert!(!error && action == BalloonAction::Update);
        let (title, _, _, action) = check_answer_balloon(&EN, &found, UpdateMode::Portal);
        assert_eq!(
            title,
            "Hoppy 0.8.0 is available — click “Download Hoppy 0.8.0…”"
        );
        assert!(action == BalloonAction::Portal);
        let failed = CheckAnswer::Failed("timeout".into());
        let (title, text, error, _) = check_answer_balloon(&IT, &failed, UpdateMode::Setup);
        assert_eq!(
            (title.as_str(), text.as_str(), error),
            (
                "Non riesco a controllare gli aggiornamenti",
                "timeout",
                true
            )
        );
    }

    #[test]
    fn about_dialog_and_menu() {
        let view = about_dialog(&IT, "0.8.0", false);
        assert_eq!(view.title, "Hoppy");
        assert_eq!(
            view.text,
            "Versione 0.8.0\ndi Marco Ricupero\n\nhttps://github.com/Marketto/hoppy"
        );
        assert_eq!(view.default_id(), BTN_CLOSE);
        let labels: Vec<&str> = view.buttons.iter().map(|b| b.1.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Chiudi",
                "Controlla aggiornamenti",
                "Pagina GitHub",
                "Diagnostica"
            ]
        );
        // Dev builds never check: no such button, and the version says so.
        let dev = about_dialog(&EN, "0.0.0-dev", true);
        assert!(
            dev.text
                .starts_with("Version 0.0.0-dev · development\nby Marco Ricupero")
        );
        let ids: Vec<u32> = dev.buttons.iter().map(|b| b.0).collect();
        assert_eq!(ids, [BTN_CLOSE, BTN_GITHUB, BTN_DIAGNOSTICS]);
        assert_eq!(dev.default_id(), BTN_CLOSE);

        // The menu has "About…" (no separate update entry) right before Quit.
        let menu = TrayMenu {
            update: None,
            firewall: None,
            music: true,
            classic: None,
            login: None,
            themes: theme_items(Theme::Night),
        };
        let items = menu.items(&EN, "Hoppy");
        let tail: Vec<String> = items
            .iter()
            .rev()
            .take(2)
            .map(|i| format!("{i:?}"))
            .collect();
        assert_eq!(tail, ["Action(3, \"Quit\")", "Action(8, \"About…\")"]);
    }

    #[test]
    fn theme_submenu_checks_the_theme_in_use() {
        let menu = TrayMenu {
            update: None,
            firewall: None,
            music: true,
            classic: None,
            login: None,
            themes: theme_items(Theme::Yume),
        };
        let items = menu.items(&IT, "Hoppy");
        let sub = items
            .iter()
            .find_map(|i| match i {
                MenuItem::Sub(label, sub) => Some((*label, *sub)),
                _ => None,
            })
            .expect("a Theme submenu");
        assert_eq!(sub.0, "Tema");
        let entries: Vec<(u32, &str, bool)> = sub
            .1
            .iter()
            .map(|i| match *i {
                MenuItem::Check(id, label, on) => (id, label, on),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries.iter().filter(|e| e.2).count(), 1);
        assert_eq!(entries[3], (ID_THEME_FIRST + 3, "Yume", true));
        for (id, label, _) in entries {
            assert_eq!(theme_of_id(id).map(Theme::label), Some(label));
        }
        // Ids around the range belong to other entries.
        assert_eq!(theme_of_id(ID_THEME_FIRST - 1), None);
        assert_eq!(theme_of_id(ID_THEME_FIRST + 5), None);
        assert_eq!(theme_of_id(ID_TRUSTED_FIRST), None);
        for id in [
            ID_SEND,
            ID_OPEN,
            ID_QUIT,
            ID_MUSIC,
            ID_UPDATE,
            ID_FIREWALL,
            ID_CLASSIC,
            ID_ABOUT,
            ID_LOGIN,
            ID_TRUSTED,
        ] {
            assert_eq!(theme_of_id(id), None);
        }
    }

    #[test]
    fn send_header_names_the_first_file() {
        let one = [PathBuf::from("/x/foto-mare.jpg")];
        assert_eq!(
            send_header(&IT, &one, 1536),
            "Invia «foto-mare.jpg» (1.5 KB) a:"
        );
        let three = [
            PathBuf::from("/x/foto-mare.jpg"),
            PathBuf::from("/x/b.txt"),
            PathBuf::from("/x/c.txt"),
        ];
        assert_eq!(
            send_header(&EN, &three, 5 * 1024 * 1024),
            "Send “foto-mare.jpg” +2 (5.0 MB) to:"
        );
        // Long, bidi-spoofed or control-character names are cleaned and cut.
        let odd = [PathBuf::from(format!(
            "/x/{}\u{202E}gpj.exe\n",
            "a".repeat(100)
        ))];
        let h = send_header(&EN, &odd, 1);
        assert!(!h.contains('\u{202E}') && !h.contains('\n'), "{h}");
        assert!(
            h.contains(&format!("{}…", "a".repeat(HEADER_FILE_MAX - 1))),
            "{h}"
        );
        assert_eq!(send_header(&EN, &[], 0), "Send “?” (0 B) to:");
    }

    #[test]
    fn fill_is_single_pass() {
        assert_eq!(
            fill(
                "Invio a {name}… {pct}%",
                &[("name", "{pct}"), ("pct", "42")]
            ),
            "Invio a {pct}… 42%"
        );
        assert_eq!(fill("{a} {missing} {", &[("a", "x")]), "x {missing} {");
    }

    #[test]
    fn peer_labels() {
        let peer = Peer {
            device: DeviceInfo {
                id: "x".into(),
                name: "Pixel & co".into(),
                platform: "android".into(),
            },
            addr: "10.0.0.2:47471".parse().unwrap(),
            crypto: Some(1),
        };
        assert_eq!(peer_label(&peer), "Pixel & co · Android");
        let spoof = Peer {
            device: DeviceInfo {
                id: "y".into(),
                name: "Mario\u{202E}\u{0}".into(),
                platform: "\u{2066}toaster".into(),
            },
            ..peer
        };
        assert_eq!(peer_label(&spoof), "Mario · toaster");
    }

    #[test]
    fn offer_body_shows_cleaned_text() {
        let file = |name: &str| hoppy_core::protocol::FileEntry {
            name: name.into(),
            size: 1,
        };
        let offer = Offer {
            sender: DeviceInfo {
                id: "x".into(),
                name: "Pixel\u{202E}\n".into(),
                platform: "android\u{200F}".into(),
            },
            files: vec![file("../../a\u{202E}gpj.exe"), file("..")],
        };
        let body = offer_body(&EN, &offer, "10.0.0.9:1".parse().unwrap());
        assert_eq!(
            body,
            "Pixel (android, 10.0.0.9) wants to send you 2 file(s), 2 B:\n• agpj.exe\n• (?)"
        );
    }

    fn session(trust: Option<TrustState>, platform: &str) -> SessionInfo {
        SessionInfo {
            encrypted: trust.is_some(),
            peer: trust.map(|_| PeerKey {
                id: "id-pixel".into(),
                name: "Pixel".into(),
                platform: platform.into(),
                fingerprint: "ABCDEFGHIJKL".into(),
            }),
            trust,
            sas: trust.map(|_| hoppy_core::Sas::from_value(0)),
            name_matches_known: false,
        }
    }

    fn offer() -> Offer {
        Offer {
            sender: DeviceInfo {
                id: "id-pixel".into(),
                name: "Pixel".into(),
                platform: "android".into(),
            },
            files: vec![hoppy_core::protocol::FileEntry {
                name: "a.jpg".into(),
                size: 3,
            }],
        }
    }

    #[test]
    fn security_notes() {
        let plain = SessionInfo::plaintext();
        assert_eq!(
            security_note(&EN, &plain, "Pixel"),
            "⚠ Not encrypted: Pixel runs a version of Hoppy without encryption"
        );
        assert_eq!(
            security_note(&IT, &session(Some(TrustState::New), "android"), "Pixel"),
            "🔒 Cifrato · dispositivo nuovo. Per essere sicuro che sia davvero Pixel, controlla \
             che sul suo schermo ci siano le stesse 4 immagini:"
        );
        let mut verified = session(Some(TrustState::Verified), "android");
        assert_eq!(
            security_note(&EN, &verified, "Pixel"),
            "🔒 Encrypted · ✓ Pixel is a verified device"
        );
        verified.name_matches_known = true;
        assert!(security_note(&EN, &verified, "Pixel").ends_with(EN.sec_name_clash));
    }

    #[test]
    fn accept_question_offers_the_emoji_code() {
        let from: SocketAddr = "10.0.0.9:1".parse().unwrap();
        let new = session(Some(TrustState::New), "android");
        let view = offer_dialog(&IT, &offer(), from, &new);
        let ids: Vec<u32> = view.buttons.iter().map(|b| b.0).collect();
        assert_eq!(
            ids,
            [BTN_DECLINE, BTN_ACCEPT, BTN_MATCH],
            "Decline is the default"
        );
        assert_eq!(view.buttons[2].1, "Coincide, accetta");
        let code = view.code.as_ref().unwrap();
        assert_eq!(code.emoji, ["🐘"; 4]);
        assert_eq!(code.names, ["elefante"; 4]);
        assert_eq!(view.details.as_deref(), Some("Verifica…"));
        assert_eq!(view.timeout, Some(ASK_TIMEOUT));
        assert!(
            view.text
                .starts_with("Pixel (android, 10.0.0.9) vuole inviarti 1 file")
        );

        // Verified or plaintext: no code, no "They match".
        for s in [
            session(Some(TrustState::Verified), "android"),
            SessionInfo::plaintext(),
        ] {
            let view = offer_dialog(&EN, &offer(), from, &s);
            assert!(view.code.is_none() && view.details.is_none());
            assert_eq!(view.buttons.len(), 2);
            assert_eq!(view.default_id(), BTN_DECLINE);
        }
    }

    #[test]
    fn sender_code_window_and_warnings_default_to_closing() {
        let known = session(Some(TrustState::Known), "linux");
        let view = code_dialog(&EN, "Laptop", &known).unwrap();
        assert_eq!(view.title, "Hoppy — verify Laptop");
        assert!(
            view.text
                .starts_with("Sending to Laptop (seen before, not verified)")
        );
        assert_eq!(view.buttons[0], (BTN_CLOSE, "Later".to_owned()));
        assert_eq!(view.buttons[1], (BTN_MATCH, "They match".to_owned()));
        assert!(view.timeout.is_none());
        let it = code_dialog(&IT, "Laptop", &known).unwrap();
        assert_eq!(it.buttons[0].1, "Più tardi");
        assert_eq!(it.buttons[1].1, "Coincide");
        assert!(code_dialog(&EN, "x", &session(Some(TrustState::Verified), "linux")).is_none());
        assert!(code_dialog(&EN, "x", &SessionInfo::plaintext()).is_none());

        let warn = key_changed_dialog(&IT, "Pixel");
        assert_eq!(warn.default_id(), BTN_CLOSE);
        assert_eq!(
            warn.buttons[1],
            (BTN_FORGET, "Dimentica dispositivo".to_owned())
        );
        assert!(warn.text.contains("«Dimentica dispositivo»"));
        assert_eq!(forget_dialog(&EN, "Pixel").default_id(), BTN_CLOSE);
    }

    #[test]
    fn device_labels_show_trust() {
        let peer = |crypto| Peer {
            device: DeviceInfo {
                id: "x".into(),
                name: "Pixel".into(),
                platform: "android".into(),
            },
            addr: "10.0.0.2:47471".parse().unwrap(),
            crypto,
        };
        let entry = |verified| TrustEntry {
            fingerprint: "F".into(),
            name: "Pixel".into(),
            first_seen: 0,
            verified,
            addr: None,
        };
        assert_eq!(
            device_label(&EN, &peer(Some(1)), Some(&entry(true))),
            "✓ Pixel · Android"
        );
        assert_eq!(
            device_label(&EN, &peer(Some(1)), Some(&entry(false))),
            "🔒 Pixel · Android"
        );
        assert_eq!(
            device_label(&EN, &peer(Some(1)), None),
            "🔒 Pixel · Android"
        );
        // An old app, unless its key is remembered (then the sender insists on v1).
        assert_eq!(
            device_label(&IT, &peer(Some(0)), None),
            "Pixel · Android · ⚠ non cifrato"
        );
        assert_eq!(
            device_label(&EN, &peer(None), None),
            "Pixel · Android · ⚠ not encrypted"
        );
        assert_eq!(
            device_label(&EN, &peer(Some(0)), Some(&entry(false))),
            "🔒 Pixel · Android"
        );
    }

    #[test]
    fn trusted_devices_are_sorted_and_tell_keys_apart() {
        let mut store = TrustStore::new();
        for (id, name, fp, verified) in [
            ("b", "pixel", "ZZZZYYYYXXXX", false),
            ("a", "Mac\u{202E}", "ABCDEFGHIJKL", true),
            ("c", "Pixel", "QQ", true),
        ] {
            store.peers.insert(
                id.into(),
                TrustEntry {
                    fingerprint: fp.into(),
                    name: name.into(),
                    first_seen: 0,
                    verified,
                    addr: None,
                },
            );
        }
        let list = trusted_devices(&EN, &store);
        let labels: Vec<&str> = list.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "✓ Mac · ABCD EFGH",
                "pixel · ZZZZ YYYY · not verified",
                "✓ Pixel · QQ"
            ]
        );
        assert_eq!(list[0].id, "a");
        assert_eq!(list[0].name, "Mac");
    }

    fn quiet_discovery(name: &str, transfer_port: u16, seed: Option<u16>) -> Discovery {
        let me = DeviceInfo {
            id: format!("id-{name}"),
            name: name.into(),
            platform: "linux".into(),
        };
        let mut cfg = DiscoveryConfig::new(me, transfer_port);
        cfg.bind_port = 0;
        cfg.use_multicast = false;
        cfg.seeds = seed
            .map(|p| vec![SocketAddr::from(([127, 0, 0, 1], p))])
            .unwrap_or_default();
        Discovery::start(cfg).unwrap()
    }

    #[test]
    fn collect_peers_times_out_when_alone() {
        let alone = quiet_discovery("alone", 0, None);
        let t = Instant::now();
        let peers = collect_peers(&alone, Duration::from_millis(300), SEARCH_SETTLE);
        assert!(peers.is_empty());
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    #[test]
    fn collect_peers_returns_soon_after_the_first_peer() {
        let receiver = quiet_discovery("receiver", 47999, None);
        let sender = quiet_discovery("sender", 0, Some(receiver.local_port()));
        let t = Instant::now();
        let peers = collect_peers(&sender, Duration::from_secs(10), Duration::from_millis(200));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device.name, "receiver");
        assert!(t.elapsed() < Duration::from_secs(5));
    }
}

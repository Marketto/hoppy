<p align="center"><img src="assets/brand/mascots-preview.png" width="420" alt="Le mascotte di Hoppy"></p>

# Hoppy

Trasferisci file tra i tuoi dispositivi nel modo più semplice possibile:
**tasto destro → Invia con Hoppy → tocchi il dispositivo**. Fine.

Obiettivo: Windows, macOS, Linux, Android e iPhone, scegliendo da solo il canale più
veloce (Wi-Fi locale, Wi-Fi diretto, Bluetooth, internet), in locale o in remoto.

> **Stato: proof of concept.** Funziona tra Windows/Linux/Mac, Android e iPhone/iPad sulla stessa rete Wi-Fi.
> Il protocollo v0 **non è cifrato**: usalo solo su reti fidate (vedi `docs/decisions/0003-v0-unencrypted.md`).

## Cosa c'è oggi

| | Invio | Ricezione |
| --- | --- | --- |
| **Windows** | tasto destro su uno o più file → *Invia con Hoppy*, file trascinati sull'icona Hoppy del desktop, *Invia a → Hoppy*, o clic sull'icona → *Invia file…* | icona vicino all'orologio, con conferma Sì/No |
| **Android** | *Condividi → Hoppy* o pulsante *Invia file* | interruttore *Pronto a ricevere*, conferma da notifica |
| **Ubuntu / Linux** | tasto destro su uno o più file → *Script → Invia con Hoppy* (File/Nautilus), *Invia con Hoppy* (Dolphin, Nemo), *Apri con → Hoppy*, o clic sull'icona → *Invia file…* | icona nella barra in alto, conferma con *Accetta/Rifiuta* nella notifica |
| **macOS** (12 o successivo, Mac Intel dal 2015 e chip Apple) | Finder: tasto destro sui file → *Servizi → Invia con Hoppy*, file trascinati sull'icona di Hoppy, *Apri con → Hoppy*, o clic sul coniglietto → *Invia file…* | coniglietto nella barra dei menu, conferma *Rifiuta/Accetta* |
| **iPhone / iPad** (iOS/iPadOS 15 o successivo, installazione con Sideloadly) | *Condividi → Hoppy* da Foto, File e altre app, o pulsante *Invia file* nell'app | con Hoppy aperto e *Pronto a ricevere* attivo, conferma *Rifiuta/Accetta*; i file finiscono in *File → Sul mio iPad → Hoppy* |

Cinque temi animati da scegliere al primo avvio: **Purikura** (predefinito), **Showa**,
**Manga** (proposto con il contrasto elevato), **Yume** e **Notte** (proposto con il tema scuro).

## Provarlo

**Windows** — installa `Hoppy-Setup-x64.exe` dalla pagina Release, oppure scarica `hoppy.exe` e
`hoppyw.exe` dagli artifact della CI (o `cargo build --release`), mettili accanto a
`apps/windows/install.ps1` ed esegui:

```powershell
powershell -ExecutionPolicy Bypass -File install.ps1
```

Hoppy resta come icona nell'area di notifica, vicino all'orologio: riceve in background e,
con un clic, *Invia file…*. Per inviare da Esplora file: seleziona uno o più file, tasto
destro → **Invia con Hoppy**, poi scegli il dispositivo dal menu. Su Windows 11 la voce è
subito visibile se all'installazione lasci attiva l'opzione «menu classico» (si cambia anche
dal menu dell'icona); altrimenti è sotto *Mostra altre opzioni* (o Maiusc+tasto destro).
In alternativa trascina i file sull'icona **Hoppy** del desktop (doppio clic: scegli i file),
oppure usa *Invia a → Hoppy*. Nessun permesso di amministratore, niente Microsoft Store.

Se qualcosa non va, *Informazioni e diagnostica…* nel menu dell'icona apre il registro
(`%LOCALAPPDATA%\Hoppy\hoppy.log`, su Linux `~/.local/share/hoppy/hoppy.log`, su Mac
`~/Library/Logs/Hoppy/hoppy.log`; da terminale: `hoppy log`).

**Ubuntu** (22.04 o successivo) — scarica `Hoppy-linux-amd64.deb` dalla pagina Release e aprilo
con *App Center* (o `sudo apt install ./Hoppy-linux-amd64.deb`). Avvia **Hoppy** dalle
applicazioni una volta: compare l'icona nella barra in alto (si riavvia da sola a ogni accesso)
e il tasto destro sui file in *File* offre **Script → Invia con Hoppy**. Scegli il dispositivo
dalla lista; l'avanzamento è nella notifica, con *Annulla invio*. Quando qualcuno ti invia
dei file arriva una notifica con **Accetta / Rifiuta**; i file vanno in `Scaricati/Hoppy`.
Su KDE la voce è direttamente nel menu del tasto destro di Dolphin. Dettagli, altri desktop e
risoluzione dei problemi: [`docs/platforms/linux.md`](docs/platforms/linux.md).

**Mac** (macOS 12 Monterey o successivo: Mac Intel dal 2015 e Mac con chip Apple) — scarica
`Hoppy-macos-universal.dmg` dalla pagina Release, aprilo e trascina **Hoppy** in
**Applicazioni**. Hoppy non è ancora firmato da Apple, quindi il primo avvio va confermato
una volta:

1. doppio clic su Hoppy in Applicazioni: macOS dice che non può verificare lo sviluppatore →
   **Fine** (non «Sposta nel Cestino»);
2. **Impostazioni di Sistema → Privacy e sicurezza** → in fondo «Hoppy è stato bloccato…» →
   **Apri comunque**, e conferma con la password. Su macOS 12–14 basta anche tasto destro su
   Hoppy → **Apri** → **Apri**; da macOS 15 Sequoia questa scorciatoia non funziona più.
3. consenti le **notifiche**, l'accesso alla **rete locale** (macOS 15) e, al primo file
   ricevuto, alla cartella **Download**.

Dopo ogni aggiornamento macOS può chiedere di nuovo questi passaggi. Il coniglietto resta nella
barra dei menu, in alto a destra, e si riapre a ogni accesso (voce *Apri all'accesso* nel suo
menu). Per inviare: nel Finder seleziona uno o più file, tasto destro → **Servizi → Invia con
Hoppy** (oppure trascinali sull'icona di Hoppy), poi scegli il dispositivo dal menu che compare
accanto al puntatore; l'avanzamento è accanto al secondo coniglietto. Quando qualcuno ti invia
dei file compare una finestra **Rifiuta / Accetta** (Invio = Rifiuta); i file vanno in
`Download/Hoppy`. Dettagli e risoluzione dei problemi:
[`docs/platforms/macos-ios.md`](docs/platforms/macos-ios.md).

**iPhone e iPad** (iOS/iPadOS 15 o successivo) — Hoppy non è sull'App Store: si installa
gratis dal PC con **Sideloadly** e il tuo Apple ID, senza account sviluppatore.

1. Sul PC Windows installa **iTunes** (versione dal sito Apple) e **Sideloadly**
   (sideloadly.io); scarica `Hoppy-ios-unsigned.ipa` dalla pagina Release.
2. Collega l'iPad con il cavo, sbloccalo e tocca **Autorizza** («Considera attendibile questo
   computer»).
3. In Sideloadly trascina il file .ipa, scegli l'iPad, scrivi il tuo **Apple ID** e premi
   **Start** (password e codice di verifica quando richiesti).
4. Sull'iPad: **Impostazioni → Generali → VPN e gestione dispositivi** → il tuo Apple ID →
   **Autorizza**.
5. **Impostazioni → Privacy e sicurezza → Modalità sviluppatore** → attiva, riavvia e conferma.
6. Apri Hoppy e consenti l'accesso alla **rete locale** (senza, non trova nessun dispositivo).

La firma gratuita **dura 7 giorni**: poi Hoppy non si apre più e basta ripetere il punto 3 (i
file ricevuti restano). Hoppy riceve **solo mentre è aperto**: lascia attivo *Pronto a
ricevere*, accetta con **Accetta** (Rifiuta è la scelta predefinita) e trovi i file in *File →
Sul mio iPad → Hoppy*. Per inviare: *Invia file* nell'app, oppure *Condividi → Hoppy* da Foto o
File. Dettagli, limiti e risoluzione dei problemi: [`docs/platforms/ios.md`](docs/platforms/ios.md).

**Android** — installa l'APK `hoppy-android` dagli artifact della CI.

**Da riga di comando** (qualsiasi sistema con Rust):

```bash
cargo run -p hoppy -- receive            # su un dispositivo
cargo run -p hoppy -- send foto.jpg      # sull'altro: scegli il destinatario dalla lista
```

## Per chi sviluppa (umani e agenti AI)

- Agenti AI: partire da [`AGENTS.md`](AGENTS.md) (router dei compiti → skill → verifica).
- Un solo comando per tutto: `python3 tools/dev.py check` (vedi `python3 tools/dev.py`).
- Protocollo: [`docs/protocol.md`](docs/protocol.md) · Architettura: [`docs/architecture.md`](docs/architecture.md) ·
  Roadmap: [`docs/roadmap.md`](docs/roadmap.md) · Temi: [`docs/design/themes.md`](docs/design/themes.md).

Licenza: MIT.

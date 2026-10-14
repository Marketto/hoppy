<p align="center"><img src="assets/brand/mascots-preview.png" width="420" alt="Le mascotte di Hoppy"></p>

# Hoppy

Trasferisci file tra i tuoi dispositivi nel modo più semplice possibile:
**tasto destro → Invia a → Hoppy → tocchi il dispositivo**. Fine.

Obiettivo: Windows, macOS, Linux, Android e iPhone, scegliendo da solo il canale più
veloce (Wi-Fi locale, Wi-Fi diretto, Bluetooth, internet), in locale o in remoto.

> **Stato: proof of concept.** Funziona tra Windows/Linux e Android sulla stessa rete Wi-Fi.
> Il protocollo v0 **non è cifrato**: usalo solo su reti fidate (vedi `docs/decisions/0003-v0-unencrypted.md`).

## Cosa c'è oggi

| | Invio | Ricezione |
| --- | --- | --- |
| **Windows** | *Invia a → Hoppy* da Esplora file | in background, con conferma Sì/No |
| **Android** | *Condividi → Hoppy* o pulsante *Invia file* | interruttore *Pronto a ricevere*, conferma da notifica |
| **Linux / macOS** | `hoppy send` | `hoppy receive` |

Cinque temi animati da scegliere al primo avvio: **Purikura** (predefinito), **Showa**,
**Manga** (proposto con il contrasto elevato), **Yume** e **Notte** (proposto con il tema scuro).

## Provarlo

**Windows** — scarica `hoppy.exe` dagli artifact della CI (o `cargo build --release`), mettilo
accanto a `apps/windows/install.ps1` ed esegui:

```powershell
powershell -ExecutionPolicy Bypass -File install.ps1
```

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

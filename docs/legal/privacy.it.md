# Informativa privacy

**Versione 1 — 4 novembre 2026**

In breve: **Hoppy non ha account, non contiene pubblicità, statistiche d'uso o
tracciamento e non invia nessun dato all'autore.** I tuoi dati restano sui tuoi dispositivi;
sulla rete locale vengono condivisi solo quelli che servono a trovare i dispositivi e a
consegnare i file che scegli di inviare. Gli unici contatti con internet sono il controllo e
il download degli aggiornamenti da GitHub.

Questa informativa descrive le app Hoppy (Windows, macOS, Linux, Android, iPhone e iPad), il
programma `hoppy` e il sito di download, secondo il Regolamento (UE) 2016/679 («GDPR»).

## 1. Chi è l'autore e come contattarlo

Hoppy è scritto da Marco Ricupero («l'autore»). Per domande su questa informativa apri una
segnalazione su <https://github.com/Marketto/hoppy/issues>; le segnalazioni sono
**pubbliche**, quindi non scrivere dati personali.

L'autore **non raccoglie, non riceve e non conserva** dati personali degli utenti di Hoppy:
non ha server che partecipano ai trasferimenti. I dati descritti al punto 2 sono trattati
dal tuo dispositivo, sotto il tuo controllo; per quelli dei punti 4 e 5 il titolare è
GitHub.

## 2. Dati conservati sul tuo dispositivo

Hoppy salva solo sul dispositivo, senza copiarli altrove:

- **nome del dispositivo** (quello del computer o del telefono, modificabile) e un
  **identificativo casuale** (UUID) generato al primo avvio;
- una **chiave crittografica** del dispositivo, creata al primo avvio, che non esce mai dal
  dispositivo e non finisce nei backup;
- l'**elenco dei dispositivi fidati**: per ognuno identificativo, nome, impronta della
  chiave, data del primo contatto, se lo hai verificato e l'ultimo indirizzo di rete usato;
- le **impostazioni**: tema, musica e suoni, ricezione automatica, data dell'ultimo controllo
  aggiornamenti e simili;
- sui computer, un **registro diagnostico** (`hoppy.log`, al massimo circa 512 KB) con
  eventi tecnici, numeri e dimensioni dei file, nomi e indirizzi IP dei dispositivi;
  **non contiene mai nomi di file, percorsi o contenuti**. Le app per Android, iPhone e iPad
  non scrivono un registro proprio;
- i **file che ricevi** e accetti.

| Dispositivo | Impostazioni, chiave, dispositivi fidati | Registro | File ricevuti |
| --- | --- | --- | --- |
| Windows | `%LOCALAPPDATA%\Hoppy` | `%LOCALAPPDATA%\Hoppy\hoppy.log` | `Download\Hoppy` |
| macOS | `~/Library/Application Support/hoppy` | `~/Library/Logs/Hoppy` | `~/Downloads/Hoppy` |
| Linux | `~/.config/hoppy` | `~/.local/share/hoppy` | `~/Scaricati/Hoppy` (`~/Downloads/Hoppy`) |
| Android | memoria privata dell'app | — | `Download/Hoppy` |
| iPhone e iPad | memoria privata dell'app | — | File → Su iPhone/iPad → Hoppy |

**Come cancellarli.** Su Android, iPhone e iPad disinstallare l'app cancella le
impostazioni, la chiave e l'elenco dei dispositivi fidati (su iPhone e iPad anche i file
ricevuti, se non li hai spostati); su Android i file ricevuti restano in `Download/Hoppy`
finché non li elimini. Sui computer disinstalla Hoppy e poi elimina le cartelle della
tabella; su Linux e macOS anche le voci di avvio automatico e del menu del tasto destro che
`hoppy uninstall` rimuove. Puoi anche dimenticare un singolo dispositivo dal menu
*Dispositivi fidati*.

## 3. Dati condivisi sulla rete locale

Per farsi trovare, Hoppy annuncia periodicamente sulla rete locale, a **tutti i dispositivi
collegati alla stessa rete**, il nome del dispositivo, il suo identificativo casuale, il
sistema operativo e la porta di rete; chi è sulla rete vede anche il tuo indirizzo IP. Questi
annunci non sono cifrati. Le app Android, iPhone e iPad lo fanno solo mentre sono aperte o
pronte a ricevere.

Quando invii file:

- i file vanno **solo al dispositivo che scegli** e **solo dopo che chi riceve accetta**;
- prima dell'invio il dispositivo che riceve vede il nome del tuo dispositivo, i nomi e le
  dimensioni dei file, e la chiave pubblica del tuo dispositivo (la sua «impronta»); se chi
  riceve accetta o verifica il codice, il suo dispositivo la memorizza nell'elenco dei
  dispositivi fidati (e il tuo fa lo stesso con la sua);
- con le versioni attuali (protocollo v1) nomi, dimensioni e contenuto dei file sono
  **cifrati da un dispositivo all'altro**; con app vecchie (v0) viaggiano **in chiaro** e
  l'app lo segnala con ⚠: chiunque sulla stessa rete potrebbe leggerli.

Nulla di tutto questo passa da internet o dall'autore.

## 4. Controllo e download degli aggiornamenti (GitHub)

Le app controllano se esiste una nuova versione al massimo ogni poche ore (sui computer anche
all'avvio, al risveglio e all'apertura del menu, con limiti di frequenza) chiedendo a
`api.github.com` l'ultima versione pubblicata. Quando scegli di aggiornare, il file viene
scaricato da `github.com`, che lo serve da domini come `objects.githubusercontent.com`. Le
richieste contengono solo i dati tecnici di una normale connessione web: il tuo **indirizzo
IP**, l'ora e la versione di Hoppy (nell'intestazione *User-Agent*). Nessun identificativo
del dispositivo o dell'utente viene inviato. Le versioni di sviluppo non fanno il controllo.

GitHub tratta questi dati come titolare autonomo, secondo la propria
[informativa privacy](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement).
Se non vuoi questi contatti puoi bloccarli con un firewall; Hoppy continua a funzionare
sulla rete locale.

## 5. Sito di download

Il sito di download è ospitato da **GitHub Pages**, che registra dati tecnici delle visite
(per esempio l'indirizzo IP) secondo l'informativa di GitHub. La pagina chiede a
`api.github.com` l'ultima versione disponibile; i font sono ospitati sul sito stesso. Il
sito **non usa cookie** né strumenti di statistica; nella memoria del browser
(`localStorage`) salva solo la lingua scelta e se hai acceso la musica, e puoi cancellarli
dalle impostazioni del browser.

## 6. Strumenti di terze parti

Per installare Hoppy su iPhone e iPad puoi usare strumenti di terze parti come Sideloadly o
SideStore (che legge l'elenco delle versioni dal sito di download e scarica l'app da
GitHub); l'installazione e gli aggiornamenti di Android passano dal sistema operativo. Questi
strumenti, i negozi e i sistemi operativi hanno le proprie informative privacy, che ti
invitiamo a leggere.

## 7. Basi giuridiche e conservazione

- I dati del punto 2 sono trattati sul tuo dispositivo **su tua richiesta**, per far
  funzionare Hoppy, e restano finché non li cancelli; il registro diagnostico si sovrascrive
  da solo (un file più una copia di riserva).
- Gli annunci e i trasferimenti del punto 3 avvengono **su tua richiesta**, quando apri Hoppy
  o invii file; il dispositivo che riceve conserva il tuo nome e la tua impronta finché chi
  lo usa non ti dimentica.
- Il controllo degli aggiornamenti si basa sul **legittimo interesse** a ricevere correzioni,
  anche di sicurezza; i dati sono conservati da GitHub secondo la sua informativa.

## 8. I tuoi diritti

Hai i diritti previsti dagli articoli 15–22 del GDPR (accesso, rettifica, cancellazione,
limitazione, portabilità, opposizione). Poiché l'autore non ha dati tuoi, li eserciti
direttamente sul dispositivo (cambiando il nome, dimenticando dispositivi, cancellando le
cartelle del punto 2) e, per i dati del punto 4 e 5, nei confronti di GitHub. Hai anche il
diritto di presentare reclamo all'autorità di controllo, in Italia il Garante per la
protezione dei dati personali (<https://www.garanteprivacy.it>).

## 9. Minori

Hoppy non raccoglie dati di nessuno, minori compresi. I minori di 14 anni dovrebbero usarlo
con l'aiuto di un adulto.

## 10. Modifiche

Se questa informativa cambia, la nuova versione ha un nuovo numero e una nuova data e l'app
te la mostra di nuovo. Le versioni precedenti restano nella cronologia del repository del
progetto.

# Privacy policy

**Version 1 — 4 November 2026**

In short: **Hoppy has no accounts, no advertising, no usage statistics or tracking, and sends
no data to the author.** Your data stays on your devices; on the local network only what is
needed to find devices and to deliver the files you choose to send is shared. The only
contacts with the internet are checking for and downloading updates from GitHub.

This policy covers the Hoppy apps (Windows, macOS, Linux, Android, iPhone and iPad), the
`hoppy` program and the download site, under Regulation (EU) 2016/679 ("GDPR").

## 1. Who the author is and how to reach him

Hoppy is written by Marco Ricupero ("the author"). For questions about this policy open an
issue at <https://github.com/Marketto/hoppy/issues>; issues are **public**, so do not include
personal data.

The author **does not collect, receive or keep** personal data of Hoppy users: he has no
servers taking part in transfers. The data described in section 2 is processed by your
device, under your control; for the data of sections 4 and 5 the controller is GitHub.

## 2. Data kept on your device

Hoppy stores only on the device, without copying it anywhere:

- the **device name** (the computer's or phone's, can be changed) and a **random identifier**
  (UUID) created on first start;
- a **cryptographic key** of the device, created on first start, which never leaves the
  device and is never included in backups;
- the **list of trusted devices**: for each one its identifier, name, key fingerprint, date
  of first contact, whether you verified it and the last network address used;
- the **settings**: theme, music and sounds, automatic receiving, time of the last update
  check and the like;
- on computers, a **diagnostic log** (`hoppy.log`, at most about 512 KB) with technical
  events, numbers and sizes of files, device names and IP addresses; it **never contains file
  names, paths or contents**. The Android, iPhone and iPad apps do not write a log of their
  own;
- the **files you receive** and accept.

| Device | Settings, key, trusted devices | Log | Received files |
| --- | --- | --- | --- |
| Windows | `%LOCALAPPDATA%\Hoppy` | `%LOCALAPPDATA%\Hoppy\hoppy.log` | `Downloads\Hoppy` |
| macOS | `~/Library/Application Support/hoppy` | `~/Library/Logs/Hoppy` | `~/Downloads/Hoppy` |
| Linux | `~/.config/hoppy` | `~/.local/share/hoppy` | `~/Downloads/Hoppy` |
| Android | the app's private storage | — | `Download/Hoppy` |
| iPhone and iPad | the app's private storage | — | Files → On My iPhone/iPad → Hoppy |

**How to delete it.** On Android, iPhone and iPad uninstalling the app deletes the settings,
the key and the list of trusted devices (on iPhone and iPad also the received files, unless
you moved them); on Android received files stay in `Download/Hoppy` until you delete them. On
computers uninstall Hoppy and then delete the folders in the table; on Linux and macOS also
the autostart and right-click entries that `hoppy uninstall` removes. You can also forget a
single device from the *Trusted devices* menu.

## 3. Data shared on the local network

To be found, Hoppy periodically announces on the local network, to **every device connected
to the same network**, the device name, its random identifier, the operating system and the
network port; anyone on the network also sees your IP address. These announcements are not
encrypted. The Android, iPhone and iPad apps do this only while they are open or ready to
receive.

When you send files:

- the files go **only to the device you pick** and **only after the recipient accepts**;
- before the transfer the receiving device sees your device name, the names and sizes of the
  files, and your device's public key (its "fingerprint"); if the recipient accepts or
  verifies the code, their device remembers it in its list of trusted devices (and yours does
  the same with theirs);
- with current versions (protocol v1) file names, sizes and contents are **end-to-end
  encrypted** between the two devices; with old apps (v0) they travel **in clear** and the app
  marks this with ⚠: anyone on the same network could read them.

None of this goes through the internet or the author.

## 4. Checking for and downloading updates (GitHub)

The apps check whether a new version exists at most every few hours (on computers also at
start, after waking up and when the menu opens, with rate limits) by asking `api.github.com`
for the latest published version. When you choose to update, the file is downloaded from
`github.com`, which serves it from domains such as `objects.githubusercontent.com`. The
requests contain only the technical data of an ordinary web connection: your **IP address**,
the time and the Hoppy version (in the *User-Agent* header). No device or user identifier is
sent. Development builds do not check.

GitHub processes this data as an independent controller, under its own
[privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement).
If you do not want these contacts you can block them with a firewall; Hoppy keeps working on
the local network.

## 5. Download site

The download site is hosted by **GitHub Pages**, which logs technical data of visits (for
example the IP address) under GitHub's privacy statement. The page asks `api.github.com` for
the latest available version; the fonts are hosted on the site itself. The site **uses no
cookies** and no analytics; in the browser's storage (`localStorage`) it keeps only the
language you picked and whether you turned the music on, which you can clear from the
browser's settings.

## 6. Third-party tools

To install Hoppy on iPhone and iPad you can use third-party tools such as Sideloadly or
SideStore (which reads the list of versions from the download site and downloads the app from
GitHub); installing and updating on Android goes through the operating system. These tools,
the stores and the operating systems have their own privacy policies, which we encourage you
to read.

## 7. Legal bases and retention

- The data of section 2 is processed on your device **at your request**, to make Hoppy work,
  and stays until you delete it; the diagnostic log overwrites itself (one file plus one
  backup copy).
- The announcements and transfers of section 3 happen **at your request**, when you open
  Hoppy or send files; the receiving device keeps your name and fingerprint until its user
  forgets you.
- Update checks are based on the **legitimate interest** in receiving fixes, including
  security fixes; the data is kept by GitHub under its privacy statement.

## 8. Your rights

You have the rights of articles 15–22 GDPR (access, rectification, erasure, restriction,
portability, objection). Since the author holds no data about you, you exercise them directly
on the device (changing the name, forgetting devices, deleting the folders of section 2) and,
for the data of sections 4 and 5, with GitHub. You also have the right to lodge a complaint
with a supervisory authority, in Italy the Garante per la protezione dei dati personali
(<https://www.garanteprivacy.it>).

## 9. Children

Hoppy collects no data about anyone, children included. Children under 14 should use it
with the help of an adult.

## 10. Changes

If this policy changes, the new version gets a new number and a new date and the app shows it
to you again. Earlier versions remain in the history of the project's repository.

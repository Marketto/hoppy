# Linux (and macOS) desktop

The `hoppy` CLI works today on Linux and macOS (`send`, `receive`, `peers`, `name`);
`install` is Windows-only. Shell integration is planned:

- Nautilus: a script in `~/.local/share/nautilus/scripts/Hoppy` running
  `hoppy send --pause "$@"` in a terminal.
- Dolphin: a service menu `.desktop` file with `Exec=konsole -e hoppy send --pause %F`.
- macOS: Share extension + menu bar app (v1).

# Privacy

_Last updated: 2026-10-29_

Diorama does not collect, send or sell any information about you. It has no accounts, no telemetry, no analytics and no crash reporting, and its developers run no servers.

## Network use

- **Update checks**: only when you click **Check for updates** (Settings → Updates), Diorama downloads a small file describing the latest release from GitHub (`github.com/karmanya007/Diorama/releases`), and, if you accept an update, the installer. Like any download, this shows GitHub your IP address; [GitHub's privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement) applies.
- **Web wallpapers** are offline by default. If you allow a web wallpaper to use the internet, that page can contact any site its author chose and send it whatever the page can see (see below). The wallpaper's author, not Diorama, decides what it collects.
- Nothing else in Diorama uses the network.

## What wallpapers can see

To work like Wallpaper Engine wallpapers, scenes and web wallpapers may receive, on your computer only:

- the cursor position and clicks on the desktop;
- the loudness of the sound your PC is playing, as frequency levels (not the sound itself; no microphone is used);
- what media is playing (title, artist, album, cover art) when a wallpaper asks for it;
- the time of day, your language, and the wallpaper's own settings;
- for some web widgets, CPU, memory and disk usage.

This stays on your computer unless you allow a web wallpaper to use the internet.

## What is stored on your computer

- `%APPDATA%\Diorama`: your settings, library folders, playlists and logs (`dioramad.log`, `diorama-webhost.log`).
- `%LOCALAPPDATA%\Diorama`: the program, caches (compiled shaders, converted videos, cost measurements) and web wallpaper storage (`webview`).

Nothing from these folders is sent anywhere. Delete them to remove all of Diorama's data.

## Contact

Questions: open an issue at [github.com/karmanya007/Diorama/issues](https://github.com/karmanya007/Diorama/issues). Security problems: see [SECURITY.md](SECURITY.md).

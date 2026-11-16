# Changelog

## 0.38.1

- **Link no longer makes the pointer stutter along the screen's edges.** With Link on, every mouse
  movement that pushed against an outer edge of the desktop — reaching for the island at the top,
  most of all — was swallowed and replayed by Link, which made the pointer (and the island opening
  under it) lag and sometimes stick. The outer edges are now left to Windows; only an edge with
  another screen behind it is Link's business.
- **Link's settings file is no longer rewritten every two seconds.** A linked PC with more than one
  network adapter is heard from several addresses in turn, and each change was saved — while
  everything else that needed Link's state, the island's window included, waited for the disk.
  The address is now kept from the connection itself, and saving happens on a thread of its own.
- If the window ever stops answering for more than a second, `phemio.log` now says for how long
  and what it was doing (lines starting with `stall:`).

## 0.38.0

- **Play on the linked PC's speakers.** With two PCs linked and *Play on several* on (Devices → Sound
  output), the other PC shows up in the list as one more speaker: tick it and this PC's sound also plays over
  there — with its own volume, delay, Auto-sync and Calibrate, and in the group volume. Uncompressed and
  encrypted, on its own connection; both PCs need 0.38. *Settings → General → Play sound from linked PCs*
  switches it off on a PC whose speakers shouldn't be used.
- The Sound output list no longer draws (or takes clicks) over its header when scrolled.

## 0.37.0 — Phemio

- **New name and look: Phemio – Your desktop cameo.** A new logo (a neon trumpet whose tube becomes the
  island's pill) for the app, tray and Settings.
- **Settings restyled** in the logo's colours: black glass and neon cyan, in dark and light themes.
  The sidebar's text is sized to fit (long names in some languages get a size smaller instead of
  running out).
- **Smoother Settings:** the selected tab's pill glides between tabs, pages fade in, the mouse wheel
  scrolls smoothly, and switches slide.
- Upgrading from DewDrop or Marisland moves settings and data over automatically.
- Source on GitHub, dual-licensed MIT or Apache-2.0; builds by GitHub Actions.

## 0.36.1

- Link finds other PCs quickly again: asking PCs one by one now runs beside the broadcasts instead of
  holding them up, and a Windows Firewall rule can be added from the Link page or Settings.

## 0.36.0 — DewDrop

- Renamed from Marisland to DewDrop; Link discovery works on networks that drop broadcasts; smoother
  page animations (frame-paced, finger-following swipes).

## 0.35.x and earlier — Marisland, Islet

- Transparency for the taskbar and File Explorer, taskbar monitor, Link, search, the lunar calendar,
  and the rest of the features described in the [user guide](docs/USER_GUIDE.md).

# Changelog

## 0.38.4

- **Link asks the network's addresses once, not over and over.** One pass when Link is switched
  on (the 254 addresses around this PC's own, a few at a time, about 6 s), and that's it — no
  more repeats while no other PC answers. **Refresh** on the Link page (next to *PCs on your
  network*) asks again when you want it to, and so does *Connect*. Broadcast discovery keeps
  listening all the time, as before, so a PC that starts Phemio later still shows up on a
  network that lets broadcasts through.
- The Link page says *No other PC found* once the pass is over, instead of *Looking…* for ever.

## 0.38.3

- **Link alone on a network no longer floods it.** While no other PC answered, Link asked every
  address of the network directly, again and again for as long as it was on — on a big network
  (a /22 has 1 021 addresses) a thousand at once, every few seconds at first and every 45 s for
  ever after. Windows has to look each address up, which crowds out its table of neighbours — the
  router among them — so everything that uses the network, the taskbar and Explorer included,
  could stall while Link was on with nobody to link to. Now: at most the 254 addresses around
  this PC's own, a few at a time, three times after Link is switched on, and that's all.
  Broadcast discovery, the paired PC's last address and a typed address work as before.

## 0.38.2

- **Link can no longer hold up the rest of Windows.** While two PCs are linked, Phemio watches the
  mouse and keyboard to know when the pointer crosses over — and Windows waits for that watcher on
  every single movement and key, for the taskbar and every app. It now never waits for anything
  itself (a busy moment lets the event through instead), and if it ever does stop answering, a
  guard takes it off within half a second and says so in `phemio.log`, instead of the taskbar and
  everything else freezing with it.
- **A link that drops straight away is no longer retried every second.** When the other PC took the
  call and hung up at once, or the connection broke right after it was made, Phemio called again
  immediately, set everything up again — the mouse and keyboard watcher included — and tore it
  down again, hundreds of times in a few minutes. Now the link only counts (and the watcher only
  starts) once the other PC has actually answered, and failed tries wait longer each time: 1, 4, 8,
  16, 32, then 60 seconds. *Connect* on the Link page tries again at once.
- The island's window never waits for Link any more: if Link is busy for a moment, it draws what
  it knew a moment ago. Switching Link on no longer does its slow work (reading the saved
  pairings, asking Windows about the screens, resetting the pointers) on the window's thread.
- `phemio.log` now says why a link ended, and notes a slow mouse/keyboard watcher or Link's state
  being held for long.

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

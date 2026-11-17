# Changelog

## 0.39.1

**Smoothness, measured on a real PC and fixed where the time went.**

Drawing
- **Frames are drawn on a Direct3D surface.** 0.39.0's hardware target never ran on real hardware
  (a frame that ended inside a clip failed, and Phemio fell back to the processor), and once that
  was fixed it measured no faster than the processor: its cost was the DC it draws through, not
  the drawing. Now Direct2D draws straight into a Direct3D 11 texture and the finished frame — or
  only the part that was drawn — is copied into the window's bitmap. Measured on that PC: the
  pieces of a frame take 0.1–1 ms whatever the window's size (an empty Settings window alone took
  over 9 ms before), and copying a frame back about 1 ms. How windows are shown is unchanged.
  Self-timing, the automatic fallback to the processor and the setting work as in 0.39.0.
- **Clips are kept track of.** A frame can no longer fail because a clip was left on (or taken
  off twice); `phemio.log` names the surface if drawing code ever does that.
- **The frosted panel behind the page icons is made once**, for a band a little taller than the
  panel, and reused while the panel slides in — not blurred again for every frame.
- **`Phemio.exe --gfx-test`** draws test pictures both ways, compares them, times the pieces of a
  frame and writes `gfx-test.txt` to the data folder.

Scrolling text
- **Glides between pixels** instead of stepping from one to the next (uneven little steps are
  what showed as judder), and rests on the pixels, crisp, when it pauses.
- **In Settings only the scrolling lines are drawn again** for their frames — not the whole
  window, 25 times a second.
- **A value that changes no longer restarts its line** (*Memory in use* jumped back to the start
  every time the megabytes changed): the scroll belongs to the line's place, and the number is
  read once a second.

Taskbar monitor
- **No more hitch once a second.** Each update moved and re-stacked the monitor inside the
  taskbar, which goes through Explorer and waits for it — measured: 30–60 ms, on the thread that
  draws everything. It is now only moved when its place or size really changed.
- The taskbar's colour (for the solid background) is read from the screen on a thread of its own.
- **Upload speed, Download speed, Disk read and Disk write are four items**, each to show, hide,
  order and pad on its own. Your settings carry over (which were on, their order and padding).
- **Padding** rows sit indented under the item they belong to, with a line tying them to it, and
  move with it when ▲▼ changes the order. Padding is now saved (it was lost on restart).

Island
- While only the mascot moves on the closed island, it is drawn 30 times a second, not at the
  display's full rate.

## 0.39.0

**Phemio is now drawn by the graphics card.**

Until now every frame was rasterised on the processor (a Direct2D *software* target), on the
window thread. Measured on a real PC, a frame of the open island took about 10 ms — more than
one frame of a fast display lasts — so whenever something moved, the window thread was busy
60–90 % of the time and the mouse, and the taskbar the island is attached to, waited behind it.

- **Hardware rendering.** The Direct2D target is a hardware one: the GPU rasterises the frame and
  the finished picture is copied into the window's bitmap. Everything else is unchanged — the
  layered windows, clicks passing through transparent parts, partial redraws, the taskbar
  attachment — because only the rasteriser changed, not how windows are shown.
- **Settings → General → Draw with the graphics card** (on by default). Off draws on the processor
  exactly as before and keeps Direct3D and the GPU driver out of the process (smallest memory
  footprint).
- **It times itself once.** The first 40 big frames are drawn by the graphics card, the next 40 by
  the processor, and the faster way stays (the graphics card unless the processor is clearly
  quicker). `phemio.log` notes the result: `render: timed both — graphics card … ms, processor … ms`.
- **It falls back by itself.** No usable graphics card, a frame that fails or comes back empty three
  times in a row → the processor takes over, and the log says why. After a driver reset the
  graphics card is started again, with every uploaded picture and brush made anew.
- The frosted panel behind the page icons (the one piece that reads back what's already drawn)
  works with both.
- `diag.log` (see 0.38.5) now also says which of the two draws and how long a frame takes.

What could not be checked before release: the test machines have no real graphics card (frames
there go through a software Direct3D), so the pictures were compared pixel for pixel between the
two paths and every fallback was exercised, but the speed on real hardware is measured by the
self-timing above on your PC, not by a number in these notes.

## 0.38.6

Measured on a real PC with `diag.on`: one frame of the open island takes about 10 ms to draw
(it is drawn on the processor), on a display whose frames are shorter than that — so anything
that moved kept the window thread busy 60–90 % of the time, and the mouse waited behind it.

- **Animations leave time for the mouse.** A frame is only drawn once the window thread has been
  free for at least as long as the last frame took: about half its time stays free whatever the
  display's rate, instead of frames being drawn back to back.
- **Moving the mouse over the island no longer draws it twice.** While frames are being drawn
  anyway (the page dots swelling under the pointer…), each movement of the mouse drew the island
  once more on top of them.
- **The Link page's pointer dot** followed the mouse by redrawing the whole island at the full
  frame rate for every movement of the mouse anywhere on the desktop; it now glides at about 12
  frames a second.

## 0.38.5

- **The Link page no longer keeps Phemio — and the taskbar — busy.** With Link on and no other
  PC to link to, the page shows a shimmering *Looking…* (or *Waiting for it on the network…* for
  a paired PC that isn't there). That shimmer redrew the whole open island at the display's full
  rate for as long as it showed — measured: the window thread busy about 90 % of the time. With
  the island attached to the taskbar, Windows ties the two together, so the taskbar and Explorer
  stuttered and froze with it, and so did everything else Phemio draws. The shimmer now redraws
  about 12 times a second, and the page itself only when something on it changed (it used to be
  drawn 8 times a second regardless). Same fix for the shimmer in Search and Translate.
- For finding out what makes Phemio slow: while a file named `diag.on` is in
  `%APPDATA%\Phemio`, a line goes to `diag.log` there every two seconds — how busy the window
  thread was and with what, which threads used the processor, and what Link sees.

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

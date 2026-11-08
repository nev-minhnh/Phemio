# Phemio user guide

Everything Phemio does, page by page and setting by setting. For a short overview, downloads and
building from source, see the [README](../README.md).

Phemio was called **DewDrop** (0.36), **Marisland** (up to 0.35) and **Islet** before that. The first start of
Phemio asks a running DewDrop or Marisland to quit, moves its settings and data (`%APPDATA%\DewDrop`,
`%APPDATA%\Marisland`, the content index in `%LOCALAPPDATA%`) to `…\Phemio`, and renames the autostart
entry. Linked PCs and pairings carry over.

## Run

Double-click `Phemio.exe` (portable, no install). The first launch opens **Settings**.
Later: left-click the tray icon (or ⚙ on the expanded island) for Settings; right-click the
tray icon or the island for the quick menu. Windows SmartScreen may warn because the exe
isn't code-signed: **More info → Run anyway**.

## The island

- **Island** (floating pill) or **Notch** (attached to the top, flared corners)
- **Expanded width and height** (Settings → Island) — how big it opens
- **Attach to the taskbar** (Settings → Island): the closed island lives **inside** a taskbar at the top or
  bottom, the way the Taskbar monitor's stats do — never covered by it, moving and auto-hiding with it. Open
  it (hover / click) and it comes out over the screen (downwards from a top taskbar, upwards from a bottom
  one), still above the taskbar, and goes back in when it closes. With the **Notch** style it hangs from the
  screen's edge (rises from it, with a bottom taskbar) and is exactly as tall as the taskbar. With the island
  style, if its *Size* is taller than the taskbar, it's made just small enough to fit. Explorer restarting:
  the island shows on its own until the taskbar is back, then goes in again. Off: the placement below.
- **Closed height** (Settings → Island, 60–140%): how tall the closed island or notch is. Taller: only the
  shape grows — text and icons stay their size; shorter than normal: they shrink just enough to fit. Also
  when attached to the taskbar: there a notch takes that share of the taskbar's height (at most all of it).
- Left / Center / Right + offset, size, and a **vertical anchor**. A taskbar on the **top edge**: the island
  sits **on** the taskbar, centred in its height, like the Taskbar monitor's stats — and follows it (moved,
  resized, auto-hide sliding in and out, Explorer restarted — noticed at once, and checked again every 0.3 s),
  kept above it so it's never covered. A taskbar on another edge: the anchor applies (*Auto*, *Screen top*,
  *Below taskbar*). It also re-asserts "always on top" on every window switch
- **Hover the closed island to open the right page**: the battery (left) or energy-saver icon (right) →
  Controls; the time or date → Clock & Calendar; the music wave → Now Playing; the speaker → Devices.
  **Nothing playing:** the empty space on the left (between the battery and the time) → Now Playing; the
  battery itself still opens Controls. Each
  target is just the item itself — the empty space around them opens the last page you used other than
  those with their own way in (Controls, Clock & Calendar, Devices, and Now Playing — it has the music,
  the left space and Alt+A); never used one yet: the first of the others in your page order.
- No battery level to read (a desktop, or Windows can't tell): the island and the Controls page show a
  **battery with an ✕** instead of a percentage. (Same when you click, with *Expand on hover* off.)
- **Fit pages to the island** (Settings → Island, on by default): make the island bigger or smaller with *Expanded width / height* and everything on the pages scales evenly to fill it.
- **Minimal notch** (thin line), **mascot** — three to pick from (Settings → Island → *Mascot look*):
  - **Robot** (the default): just a face — glowing eyes, red cheeks and a mouth. It blinks, smiles, beams
    "^ ^", grins and hops, goes flat-mouthed and half-lidded when bored, round-eyed with an "o" when
    surprised, glances about, tilts its head, wanders along the island and dozes off with a drifting "z".
    Its goodbye: a wink and a big grin while a little heart floats up, then it switches off like an old TV
    (a glowing line that shrinks to a dot).
  - **Cat**: ears, eyes, rosy cheeks, whiskers and a little pink nose over a cat "w" mouth; it blinks often,
    glances and looks up, walks, hops and pounces (its whiskers wave while it's lively), flicks an ear, squints,
    tilts its head, sticks its tongue out, meows and dozes off. The mouth reacts: an "o" when surprised, open
    when it notices you, flat when bored, a big smile when happy. In **Always** mode its paws come up either
    side of its face and **drum on a little desk as you type** (one tap per key, alternating paws — it only
    learns *that* a key went down, never which), and while **music plays** it drums and sings along on the
    beat with little notes floating up. Its goodbye is a magic trick: a magician's hat drops onto the desk,
    a wand taps it twice in a shower of sparkles, the cat hops in — *poof* — a paw pops out and waves, then
    the hat flips over, hops away and twinkles out.
  - **Cat vs Robot**: the two on one island (it widens a little). They stare each other down, taunt (a hiss
    and fangs, angry red eyes and steam), and every so often crash into a cartoon brawl — a dust cloud with
    stars, bolts, a paw and a robot claw flying out — and come out dizzy. The score sits between them. Dozing:
    a truce, asleep leaning on each other. The goodbye is the **final round**: *VS*, the big brawl, the cloud
    bursts, **K.O.!** — the winner (random, every time) gets a crown and confetti, the loser is out cold with
    stars circling; the score is kept.
  - *Mascot*: **Off**, **When idle** (after 10 s – 30 min idle; any key or mouse move and it says goodbye,
    then the island comes back), or **Always** — it lives on the closed island and the island doesn't open; click the mascot and it
    says goodbye and goes, and it comes back after the *Appears after* time once you're done with the island.
- Closed island battery (left): a **ring** or a **battery icon with the percentage** (Settings → Island → *Battery look*)
- Closed island right icon when no music plays: **energy saver** or the **speaker** (click it to mute / unmute)
- Closed island: the **time** (12- or 24-hour, **with seconds** if you like) **or the date** (Settings → Island → *Closed island shows*) · focus ring + countdown · the **battery ring stays on the left** (album art only on PCs without a battery) +
  equalizer while music plays, otherwise **battery % ring** (left) and **energy saver ON/OFF**
  (right) · scroll to change volume · drop files on it
- **Text**: font (Segoe UI Variable, Segoe UI, Bahnschrift, Cascadia, Georgia or any installed
  font), size, and Normal / Bold / Italic / Bold italic
- Picking Left / Center / Right resets the offset to that spot

### Pages (expand by hover or click; switch with ‹ › arrows, the dots, ←/→, a two-finger sideways swipe, or the wheel over empty space)

**Page dock**: point at the page dots of the opened island and a small capsule dock (fully rounded ends)
slides out next to them with every page's icon — the same line icons as at the top left of the pages. The
icons swell under the pointer the way the macOS Dock does (the one you point at most, its neighbours a
little), the page you're on and the one you point at show in their colours with the rest quiet, and the
name of the one you point at appears in a small pill; click to jump straight there. Settings → Island →
*Page dock*: on **frosted glass** (the page behind blurred, no outline) or with **no background** (each icon
on a soft halo so it reads over anything). *Page dots*: the dots at the **top** or the **bottom** of the opened island.
*Page dock position*: the dock **above** or **below** the dots. Toward the page (dots at the top with the
dock below, or at the bottom with the dock above) it opens over the page; toward the island's edge (dots
at the top with the dock above, or at the bottom with the dock below) the island grows just enough for it
while it shows — the capsule and a thin strip for the name (above the icons when the dock is above, below
when it's below), with the icons swelling towards the dots — and shrinks back when it folds away.

**The red dot** at the top of the opened island quits Phemio completely (its × shows when you point at it).

| Page | |
|---|---|
| **Now Playing** | A big cover on the left (a wide video thumbnail is shown whole at its own shape, never stretched or cropped, on a soft blurred square made from it; browser web apps like YouTube Music: the song's cover, not the browser's icon); on the right title and artist, the progress bar, large controls with play in a circle of the cover's colour, and along the bottom where the sound goes plus a volume slider (click its icon to mute). The button at the top right (**↗ Spotify**, **↗ Chrome**…) — or a click on the cover — brings the playing app to the front (restored if minimised; for a browser, the window showing the song; an app closed to the tray is started). Any player (Spotify, YouTube in a browser, …): (size in Settings → Pages), title, play/pause/next/prev, click the bar to seek, output-device chip |
| **World Clock** | Your time and a second city you type in (time zone found automatically), each centred in its half, with **each city's weather right now** under it (big: icon, temperature and sky; looked up by the city's name only — if the name isn't found it says *Not found*; how often it updates is in Settings → Pages → *World Clock weather update*, 5–180 min) and, where clocks change, **when daylight saving next starts or ends** (from Windows' own time-zone data) |
| **Clock & Calendar** | 3 : 7 — on the left the time with seconds, the date, the **lunar date** (Âm lịch, with the year's and day's can chi and a little moon showing its phase) and your city; on the right a big month calendar. Switch it between **Solar** (Dương lịch) and **Lunar** (Âm lịch): either way every day shows its date in the other calendar too, small underneath (the 1st of each lunar month as "1/9", the 1st and 15th picked out). Every day is tinted in one colour at three strengths: faint for hắc đạo (unlucky) days, brighter for hoàng đạo (lucky) days, full for today. The summary under the clock shows whether today is a **Lucky / Unlucky day** (with an icon) and its spirit. Click a day (today too) to see it in full on the left (scroll it when it's long): both calendars, **ngày hoàng đạo / hắc đạo** with the day's spirit, **giờ hoàng đạo (cát)** and **giờ hắc đạo (hung)** with their hours, trực, the solar term (tiết khí), the star sign (**cung hoàng đạo**), the clashing animal (xung tuổi) and Tam nương / Nguyệt kỵ days, fading in — click the date, or the day again, and it fades back to the clock. ‹ › browse months; click the **month's name** for a month / year picker (with *Today*). Worked out on your PC for Vietnam's time zone (UTC+7), leap months included |
| **Weather** | 2 : 8 in both views — now on the left (text size in Settings → Pages → *Weather text size*; the sky icon above the temperature, sky, high/low, feels like, then humidity and wind each on its own line), on the right **Next days** or **Today**: today and tomorrow hour by hour — temperature and feels-like, humidity and chance of rain as a chart with the numbers, a dashed line at the current time and a colour legend (temperature, feels-like, humidity, chance of rain, now); point anywhere over the chart for a card with all four for the nearest hour (it glides along as you move and fades in and out). Switching *Next days* / *Today* slides the views across. Scroll the chart sideways with the wheel or two fingers (the page doesn't flip while the pointer is on it). Click the place to change it. **Forecast source**: Open-Meteo, MET Norway (yr.no), wttr.in, ECMWF or NOAA GFS — all free, no account |
| **Focus** | Fills the page: the ring on the left, the rest across the width; the ring sweeps smoothly, the highlight slides between the phase chips and the −/+ fade out when it starts. Pomodoro: click **Focus / Short break / Long break** to switch, − / + inside the ring change the length of the one shown (also in Settings → Pages). After the set number of short breaks (4 by default) comes a long break — the dots fill as you go; today's focus sessions and long breaks are counted. Chime, countdown in the closed island |
| **To-do & Notes** | A **To-do / Notes** switch at the top left. To-do: type straight into the island; `Call mum @18:30` adds a reminder. The two slide and fade into each other. Notes (2 : 8): sticky notes like Windows' — a list on the left (newest first), the note on the right; **bold, italic, underline, strikethrough**, **bulleted and numbered lists** with levels — **Tab** moves an item in a level and **Shift+Tab** back out (2-space indent; bullets go •, ◦, ▪ and numbers 1., a., i. by level), Enter continues the list and Enter on an empty item steps it out; six colours, delete. Saved as you type (`notes.json`) |
| **Shelf** | Drop files on the island, drag them out into a chat or folder (never moved). Click an item to open it; the small **Folder** button under it opens its folder with the file selected. Linked with another PC (Link page)? A second tab shows **that PC's shelf**: click an item (or *Get all*) to copy it here — it lands in `Downloads\Phemio` (or the folder set in Settings → Pages → Shelf) and on this shelf, folders included. Items already there (same name and same contents, checked by hash) show a green tick and aren't fetched again; *Get all* fetches only the rest. Once an item is here, click it again to **open** it. **Open folder** (both tabs) opens the folder those files land in |
| **Controls** | Big tiles, sliders and shortcut buttons spread over the whole page. Real on/off switches: **Wi-Fi, Bluetooth, airplane mode, keyboard, mute mic** (on = microphone muted), **dark mode** — Wi-Fi, Bluetooth, airplane mode and mute mic switch the moment you click (and switch back if Windows couldn't do it); brightness (built-in screens) and volume sliders; battery % with energy saver on/off (click → Settings › Power & battery); shortcuts to Windows' Quick Settings, the **Devices** page and Settings. Turning **Wi-Fi** or **Bluetooth** on (or the › on its tile) opens a list: **Wi-Fi networks** in range (the network you're on shows at once, the rest as the scan finds them; each row has its own **Connect** / **Disconnect** button — clicking the name only highlights the row; new networks ask for the password right in the island, work / school networks (802.1X) for a **user name and password**; while it connects the row says so, and if it fails it says so in red and the button comes back) and **Bluetooth devices** — paired ones (**Connect** / **Disconnect** button on the row, with *Connecting…* and a red *Couldn't connect* like Wi-Fi) and nearby ones ready to pair (**Pair**). Lists scroll with the wheel or a two-finger swipe. The **Keyboard** tile shows the keyboard in use (EN, VI…) and lists every keyboard installed in Windows — click one to type with it. Windows 11 24H2+ only shows apps the networks nearby when **location is on for desktop apps**; until then the
list shows only the network you're on, Windows' one-time permission prompt appears the first time, and a button
opens Settings › Privacy › Location — turn on *Location* and *Let desktop apps access your location*; the list
fills in by itself while it's open. Each list ends with a button to Windows' Wi-Fi / Bluetooth / language settings |
| **Devices** | Left: the audio output and **every connected Bluetooth device** (headphones, speakers, mice, keyboards, controllers, pens, phones…) as a deck of cards with type, Classic/LE and battery — scroll or click to flip, dots to jump — and the volume slider with a **sound output** button (pick headphones, speakers, HDMI… like Windows' own list). Right: Wi-Fi (name, band, signal, or off), Ethernet (link speed), VPNs, airplane mode, microphone, Bluetooth on/off, local IP with the **MAC address** of the adapter it's on (click either to copy it — it says *Copied* for a moment), every connected Bluetooth device with its battery (read from the same place as Windows' Bluetooth settings, refreshed every minute while the page is open) — scroll the list (wheel or two-finger swipe) |
| **Remote Desktop** | Type a computer (name or IP), user name and password and press **Connect** (or Enter): Windows' own Remote Desktop opens and signs in by itself. The password goes into Windows Credential Manager (where Remote Desktop keeps saved sign-ins) — kept with *Remember password*, removed again a minute later without it; Phemio never stores it. *Full screen* and *All screens* set how the session opens. Recent computers are listed on the right (click to fill in, × to forget), and a linked PC (Link page) is one click away |
| **Search** | The same search as Alt+S, inside the island: click the field and type; scroll the results with the wheel or two fingers |

Each page can be switched off and **moved** (▲▼) in Settings → Pages; the island follows that order.

### Play on several outputs (Devices → Sound output)
Windows plays every app to one device — the **main** output (the Windows default, tagged *Main* in the list).
Turn on **Play on several** and **tick** other devices (say the built-in speakers plus a Bluetooth speaker or
two) to add them to the group: Phemio records what the main output plays (Windows' own loopback, no driver
to install) and plays a copy on each ticked device. Off = nothing recorded, no CPU.
- **Click a device** to open its own panel (clicking no longer switches the main output): its **volume**, its
  **delay** — type the exact milliseconds in the box (or use − / + and the slider, 0–800 ms) — and its own
  **Auto-sync**, **Calibrate** and **Make main** buttons. *Make main* is the only thing that changes the main
  output.
- **Auto-sync** estimates the delay (Bluetooth speakers run about 170 ms behind, wired ones about 10 ms).
  **Calibrate** lines it up for real: it plays a chirp on the main output and on that one device and listens
  with the microphone — once with the device silent, once with it on — and sets the delay from the difference.
  Only the device you picked plays the test, so it works with two or more Bluetooth speakers too. *Auto-sync
  all* / *Calibrate all (mic)* at the bottom do the whole group. Keep the room quiet for a few seconds.
- If a device turns out *slower* than the main output, Phemio makes the slowest one the main output by
  itself and lines the others up to it — no button to press.
- **Sound input** (the button beside Sound output) picks the microphone Windows uses — and so the one
  calibration listens with. A Bluetooth speaker's own microphone is tagged *call quality*: using it drops that
  speaker into low-quality call mode, so calibration refuses it — pick the PC's microphone.
- **Volume for the group:** while devices play together, the main volume slider (Controls, Devices, the player)
  shows the group's volume — the **average** of the speakers' levels. Dragging it moves them all from where they
  were when you grabbed it: down toward 0 below the average, up toward 100 % above it, so each keeps its place and
  both ends reach every speaker. **Mute** (the speaker icon, the mute key or Windows' own) mutes them all.
  Turn Play on several off and it's the plain default-output slider again.
- The volume keys and Windows' own volume slider move every speaker by the same step (Settings → *Volume step for several speakers*, 3 % by default), each keeping its
  own level (55 / 65 / 40 → 58 / 68 / 43). The volume row (Controls, Devices and the player) ends in a small
  **speakers** button with how many play together: click it for a card with each speaker's own slider
  (click anywhere else to close it). It's only there while Play on several is on with two or more speakers
  connected; a speaker that disconnects leaves the card. The volume popup shows a row for each.
- When two outputs share a name (two "Realtek(R) Audio"), the endpoint's own name is shown instead
  ("Speakers", "Headphones").
- Clocks drift a little over time; Phemio keeps each copy at its delay by trimming a few milliseconds now
  and then. Everything the PC plays is copied (notification sounds too).

### Link: one mouse and keyboard for your PCs
**Links and Display** (the page's name now): the map's **Identify** shows each screen's number on it, linked
or not. **Click a screen** on the map for its display settings on the right — the same as Settings → System
→ Display for that screen:
- **Make this my main display** (★ *Main display* on the one that is; also **Make main** on the map), for
  this PC's screens and, when linked, the other PC's (asked over the link; both need 0.33.16).
- **Scale** (‹ 150% ›, the ones Windows offers for that screen), **Resolution** (‹ 2880×1800 ›, largest
  first; ✓ marks what Windows recommends) and **Orientation** (landscape, portrait, flipped) — for every
  screen, main or not, this PC's and (when linked) the other PC's: its settings are asked for and changed
  over the link, and its own 15-second "keep?" runs there (Keep / Revert work from here too). A new resolution or orientation asks **Keep these display settings?** and goes back by itself
  after 15 seconds unless you press *Keep changes* (or *Revert* to go back now) — like Windows does. Changing a display no
  longer throws the pointer into the middle of a screen: it stays where it was (pulled onto a screen if that
  spot is gone), unless you move it yourself meanwhile.
Click the screen again (or ×) for the Link status. Arranging and the main display use Windows' own display
configuration (what Settings uses), so they stick after a restart; if Windows refuses, the reason shows
under the settings and in `phemio.log`.

**Not linked to another PC**, the Link page's map is just this PC's screens laid out exactly as Windows has
them (Settings → System → Display), and dragging one moves it in Windows too — the same as dragging it in
Display settings: it snaps flush against the others, the main screen stays put, and the pointer crosses
between screens the new way right away. Once linked, the map is the shared one again (this PC's screens
start from the new arrangement).

Run Phemio on each PC (same local network) and open the **Link** page on the island. PCs find each
other by broadcast and multicast, and — since many Wi-Fi networks and routers drop those — also by asking
every address on this PC's network directly (and the ARP neighbours, and a paired PC's last address), so
two PCs that can reach each other by IP also find each other. That asking runs beside the broadcasts (never
holding them up), from the same port, so it gets through even a firewall that has no rule for Phemio yet.
Windows Firewall rules belong to the exe's path, so a renamed or moved Phemio.exe starts with none (or with a
block rule if the "Allow access" prompt was closed) and other PCs' broadcasts don't get in; the Link page and
Settings → General then offer **Allow**, which adds the rule (Windows asks for an administrator first). The **Link** switch (in the map's header) turns it all off — no looking for other PCs, nothing listening — until you press *Turn Link on*.
- **Scrolling over there**: the mouse wheel and a touchpad's two-finger scroll both scroll the other PC while
  the pointer is on it (the touchpad's scroll is caught by an invisible window under this PC's parked pointer).
- **Pair once:** the other PC shows up under *PCs on your network* — press **Pair**. Both PCs show the same
  6-digit code; check they match and press **Codes match** on both. (The code comes from the key exchange
  itself, so if someone tried to sit in between, the numbers wouldn't match.) After that they connect by
  themselves whenever both are on.
- **Arrange every screen on one map:** all screens of both PCs at their true relative sizes (scaling taken
  into account). Drag them to match your desk; edges snap together and screens never overlap. The map —
  not Windows' own display arrangement — decides every crossing, even between screens of the same PC, so
  the PCs' screens can be mixed in any order (e.g. laptop 1 left, laptop 2's monitors on top, laptop 1's
  monitor below them, laptop 2 right). The layout is shared, so both PCs always agree.
- **Which screen is which:** **Identify** shows a big number on every screen of both PCs for a few
  seconds — the same numbers as on the map.
- **Use it:** push the pointer through an edge where another PC's screen touches it on the map; the
  keyboard follows the pointer. Touching your own mouse or touchpad on a PC takes it back. *Go to …* jumps
  straight over. **Scroll Lock** always brings the pointer home; if the other PC stops answering, it comes
  home by itself; nothing stays pressed over there when it does.
- **Copy here, paste there:** text and pictures go to the other PC as soon as you copy them; **files and
  folders** follow the pointer — copy them, move the pointer over, press Ctrl+V (they're sent while the
  pointer crosses; big ones show their progress on the Link page, up to 4 GB at a time). Pasted files land in
  a folder of their own (cleared after a day), so Ctrl+V pastes a copy wherever you like.
- **Shelf:** each PC sees the other's shelf on the Shelf page (see above); downloads show a progress bar with
  the percentage, speed and time left. Where they go: Settings → Pages → Shelf (Downloads\Phemio by default).
- **Disconnect / Connect:** *Disconnect* hangs up and keeps the PCs apart until you press *Connect*. If the
  PCs don't find each other (some routers block it), type the other PC's **IP address** in the field on the
  page and press *Connect* — it pairs first if needed and is remembered.
- **Private:** everything after pairing is encrypted (AES-256-GCM, a new key each session; about 0.1 µs per
  mouse move, so it adds no delay) and only paired PCs are accepted. The pairing secret is stored with
  Windows' own protection (DPAPI).
- **Delay:** shown live on the page (latest round trip and the worst of the last 30 s). On wired Ethernet
  expect well under a millisecond; Wi-Fi works too but can have the odd hiccup (turning off the Wi-Fi
  adapter's power saving helps).
- Windows asks once to allow Phemio on private networks — allow it on both PCs. Ports: UDP 47830
  (finding each other), TCP 47831 (the link and files). Not yet: the lock screen and windows running as
  administrator. Turn it off in Settings → General → *Link with your other PCs*.

### Music keys (Alt+A, Alt+Z, Alt+X)
**Alt+A** opens Now Playing (it closes by itself after a few seconds unless the pointer is on it; Alt+A again
closes it), **Alt+Z** plays the previous song, **Alt+X** the next one — in whatever app is playing. Turn them
off in Settings → General → *Music with Alt+A, Alt+Z, Alt+X* (if another app already uses one of them, the
log says so). Now Playing's ⏮ ⏯ ⏭ are filled round buttons in the colours chosen in Settings → Island
(*Colour theme* / *Icon colours*), with a **repeat** button beside them: off (outlined) → the whole list →
this one song (with a "1") → off. It uses the player's own repeat when it has one; for a player without it,
Phemio repeats the one song itself (when the song ends by itself it goes back to it). Previous / next
still work normally while repeat is on. On the far left, **stop after this song** (■ in a circle): outlined,
the next song plays on as usual; filled, the song stops the moment it reaches its end — even with repeat on
— and waits at its start (press play to hear it again; next / previous still skip as usual).

### Translate the selection (Alt+Q)
Highlight text in any app and press **Alt+Q**: the island opens a card with the original above and the
translation below. Chips on the card pick who translates: **Google** (Google Translate) or a Gemini model —
**3.8 Flash**, **3.6 Flash** or **3.5 Flash Lite** (the default; Settings → General → *Translate with*).
Gemini writes the translation out as it goes, word by word (translations never use thinking — faster). It's told (behind the scenes, never part of the text it translates or of what it writes back) to understand the whole text in context — tone, slang, idioms, who's talking to whom — and say it the way a native speaker would, not word for word, while keeping every fact, number, name and nuance exactly. Copy with the copy
button, Enter or Ctrl+C; the wheel or ↑↓ scroll a long one. **Esc** or a click anywhere else closes the card
(Esc hands the keyboard back to the app you were in); otherwise it leaves by itself a few seconds after you
look away. The selected text is read through Windows' accessibility interface, or with a quick Ctrl+C whose
clipboard is put back right after. Turn it off in Settings → General → *Translate with Alt+Q*.

### Settings
- **Search settings** (top of the side bar, or just start typing / paste; Ctrl+F): every matching setting from
  all tabs, under its tab's name. Esc or ✕ clears it. Every text box in Settings edits like any other:
  ←/→ (Ctrl: a word), Home/End, Delete, click to put the caret, Ctrl+A / C / X / V.
- Animations run at the display's own pace (each frame waits for the screen's vertical blank, and steps
  are whole display frames), so they don't judder on 60, 120 or 144 Hz screens. Changing page slides the
  new page in as the old one slides out; a two-finger swipe moves the pages with your fingers and turns
  exactly one page (let go before a third of the way and it springs back).
- Sliders follow the pointer smoothly: the island shows the change as you drag, and the setting is saved once
  you let go.

### Search (Alt+S)
Press **Alt+S** anywhere: a search bar grows out of the island; type, press **Enter**, and the results fall in
underneath (Enter again opens the chosen one). Prefer results while you type? Settings → General → *Search as you type*. The empty box lists everything it can do — click a line to try it.
- **Apps** (everything in Start's *All apps*, Store apps too) are found right away, built in.
- **Files and folders** come from **[Everything](https://www.voidtools.com/downloads/)** by voidtools — Phemio
  talks to it directly, so you don't need `es.exe` or anything else; just install Everything (free) and keep it
  running (it starts with Windows by default). Without it, the search shows a button to download it (also in
  Settings → General → *Get Everything*); if it's installed but closed, a button starts it.
- **Filename | Content** (the switch beside the box) — two separate searches: *Filename* looks only at names,
  *Content* only **inside files**. Only files whose passage can be shown are results — each with the passage
  around your words, the words highlighted. Content looks for **exact words**: any case, but accents count
  (`qui` finds *qui*, not *quỉ*, *quĩ*, *quị*; `quỉ` finds only *quỉ*), whole words only. Several words: files
  with all of them (`Cộng Hoà` → *cộng* and *hoà* anywhere); in **"quotes"** they're one phrase (`"Cộng Hoà"`
  → exactly *cộng hoà*). ←/→ still edit what you typed, ↑/↓ still pick a result.
  - **Phemio's own index** (Settings → General → *Content search*): pick the drives, and Phemio reads
    the documents on them once in the background, at low priority — **Word (.docx, .doc), Excel (.xlsx, .xls),
    PowerPoint (.pptx, .ppt), PDF, OpenDocument, EPUB, RTF, e-mails, text and code** — then keeps up **as you
    work**: Windows tells Phemio whenever a file on those drives is saved, added, renamed, moved or
    deleted, and a few seconds later (once it's stopped changing) just that file is read again or taken out
    — a new folder's files too (a file that keeps changing, like a log, is read again at most once a
    minute, and the log notes these updates at most every 10 minutes). As a safety net, every half hour and when Phemio starts it also checks for
    anything missed (only **new, changed or gone** files). Searches then answer at once. Skipped:
    Windows' and programs' folders, app data, hidden/system folders, code dependency folders (node_modules…),
    cloud-only OneDrive files (so nothing gets downloaded), files over 150 MB. Settings shows how many files
    are in it and when it was updated, with **Update now**, **Stop** and **Delete** (forget it all). The index
    lives in `%LOCALAPPDATA%\Phemio\contentindex.db` and never leaves your PC. Updating Phemio
    doesn't touch it — a new version carries on with the same index (only files new or changed since are
    read). After each look it's also copied to `contentindex.backup.db` (when it changed, at most every
    10 minutes); if the index is ever lost or damaged, Phemio brings it back from that copy at start
    instead of reading everything again. *Delete* removes both.
  - With no drive chosen, *Content* asks Windows' own search index (as before), and Everything if Windows
    Search is off; files whose passage Windows' readers can't give are left out.
  While it looks, a moving bar and **Cancel** show; cancelling, a new search or closing the island stops it.
- **Edit what you typed** anywhere in it, before or after Enter (QR, Base64, sums, currencies, translations,
  Google, Gemini…): ←/→ move the caret (Ctrl: a word), Home/End, Delete, or click where you want it; ↑/↓
  still pick a result. The same goes for every text field on the island.
- **50 results to a page**: when a search finds more (file names, or inside files), the footer shows which
  ones these are (`51–100 / 312`) with ‹ › to turn the page; **PgDn / PgUp** do the same, and ↓ on the last
  row (↑ on the first) carries on to the next (previous) page. ←/→ still edit what you typed. Phemio's
  own index shows the first page at once and fills in the rest (up to 500) a moment later; Everything gives
  up to 1,000 names.
- **Long lines scroll**: any text too long for its place — on the island, in search results and in Settings —
  no longer ends in "…": it waits a moment, then glides slowly to show the rest, pauses, and starts again
  (at most 25 frames a second, and only while it moves).
- Web results are numbered 1, 2, 3… in Google's order, in a disc in your accent colour (readable in light
  and dark mode).
- Instant answers (Enter copies them). Each shows **only its own result** — a sum (`1+1-452`), a currency,
  a translation, base64, a QR code — with no files or web rows mixed in (a date such as `2024-10-23` is
  still searched as text):
  - Sums: `12*(3+4)`, `2^10`, `15%`
  - Currencies (Google Finance): `100 usd to vnd`, `$25 in eur`, `1.5m vnd usd`, or just `50 eur` → your currency
    (Settings → General → *Your currency*: VND, USD, AUD, EUR, GBP, JPY or CNY)
  - Base64, worked out on your PC: `B64(some text)` encodes, `b64d SGVsbG8=` decodes, and a pasted base64
    string offers its text by itself
  - QR codes, on your PC too: `QR(any text or link)` draws one — Enter copies it as a picture; paste a
    picture or screenshot with a QR code (Ctrl+V in the box — a sharp thumbnail of it shows in the box) to read it — Enter opens a link or copies the text
  - **Text in a picture, layout kept (OCR)**: type **`<`**, then paste a picture or screenshot (Ctrl+V) — the
    thumbnail shows in the box as for QR codes, and the text appears below as a small **document that keeps
    the picture's layout**: headings, paragraphs, bulleted and numbered lists, **tables** (as real tables,
    every row and column), several columns in reading order, bold and italic, code. It scrolls. **Drag across the text to select it** (across paragraphs, lists and table cells, like in
    a browser) — letting go copies the selection. The block under the pointer has a small **Copy** for all of
    it (a table copies as rows of cells that paste as a table into Word or Excel), and **Copy all** (or Enter)
    copies everything as Markdown. **Alt+Q** while it's showing translates it with the usual translator card
    (Google Translate or Gemini): just the selected text if you've selected some, otherwise all of it;
    closing the card brings the picture's text back. With a Gemini
    key (Settings → General) Gemini reads it (any language, Vietnamese accents included; the picture is sent
    to Google for this); without one, or if Gemini can't, Windows' own text reader does it on your PC and
    Phemio rebuilds the layout from where each word sits (headings by size, paragraphs, lists, tables by
    columns) — it reads the languages whose OCR is installed (Settings → Time & language → Language & region),
    without Vietnamese accents. Without the `<`, a pasted picture is read as a QR code, as before (typing `<`
    right after pasting reads the same picture's text).
  - Translation (Google Translate): `tr good morning`, `tr ja: good morning`, `dịch …` — into your language
    (Settings → General → *Translate into*: Vietnamese, English, Japanese, Korean, Chinese, French or Spanish), or English when it's already in your language
  - **Dictionary (Alt+W)**: press **Alt+W** anywhere and the search box becomes a dictionary (a book instead
    of the magnifier, nothing extra to type): type a word and Google Translate gives its translation, then its
    other translations laid out like Google Translate's own list: each word, its part of speech (Noun,
    Verb…) and, as small chips, what it means back in the word's language. Always into the language in Settings → General →
    *Translate into*. ↑/↓ pick a line, Enter copies it, ←/→ edit the word; Backspace in the empty box goes
    back to the normal search, Alt+W again closes it. Turn it off in Settings → General → *Dictionary with
    Alt+W*.
  - Gemini (Google AI): `? your question`, then Enter — or pick *Ask Gemini “…”* under any search. Like a
    chat app, you see Gemini **thinking** (its thoughts stream in, quiet and indented), then the answer is
    **typed out word by word** with a caret; afterwards the thinking folds into "Thought for 2.8s" (click to
    unfold). The **Think** switch on the answer shows **On** / **Off**: on asks Gemini to show its thinking
    (`thinkingConfig.includeThoughts`), off asks for as little thinking as the model allows (`thinkingLevel:
    minimal`, or `thinkingBudget: 0` on models that take that) — `phemio.log` notes which was sent (also Settings →
    General → *Gemini thinks first*). Translations never think. Chips on the answer switch between **Gemini 3.8 Flash, 3.6 Flash** (the default) and **3.5
    Flash Lite** and ask again. Long answers scroll and follow the writing until you scroll up. When your key
    allows it, Gemini looks the question up on Google Search first and lists the pages it used (click one to
    open it). Enter or *Copy* copies the answer; *Search Google* runs the same question on Google. Gemini
    always waits for Enter, even with search-as-you-type on (each question uses your quota). Needs a free API
    key from Google AI Studio (Settings → General → *Gemini API key*, with a *Test* button).
  - Google: `> your question` (or `g …`), then Enter, shows Google's **top 10 results** laid out like the
    search page — site icon and name, link, title, and the snippet. They're read off Google's real results
    page in a hidden Microsoft Edge WebView2 (part of Windows 11, nothing to install), so they're the same
    results, in the same order, as in your browser. Enter opens one, Ctrl+Enter copies its link, and *All
    results on Google* (bottom right) opens the full page. The hidden browser closes itself a minute after the
    last search (it uses ~100 MB while open). If Google's page won't load (or asks "are you a robot?"),
    Gemini answers instead, and `phemio.log` says what the page showed.
  - While the internet answers, the row "thinks" (breathing dots, a light sweeping the text).
- The wheel or a two-finger swipe scrolls the results (it no longer changes the volume).
- ↑↓ choose · Enter opens · Ctrl+Enter shows it in its folder · Esc or clicking elsewhere closes. Things you
  open often move up the list.
- Text boxes everywhere (search, Gemini key, to-do, places, addresses…): **Ctrl+A** selects all, **Ctrl+C** /
  **Ctrl+X** copy / cut it, **Ctrl+V** pastes (over the selection). Passwords are never copied.
- One Enter is enough with Vietnamese Telex and other typing methods too (the Enter that finishes the word
  also searches).
- Turn the shortcut off in Settings → General → *Search with Alt+S*.

### Look & language
- **Language**: English, Tiếng Việt, 中文, 한국어, 日本語, Français or Español (Settings → General) — menus,
  pages, alerts, dates and weekday names. Names that read the same everywhere (Dynamic Island, Windows,
  Wi-Fi, IP…) stay as they are
- **Colour theme**: Full, Cool, Cold, Warm or Hot — icons, indicators, charts and the battery ring (never plain white); **Icon colours** can instead follow the album or be Mono (Mono greys only the icons — the equaliser, player buttons, sliders and lit tiles keep their colours)
- **Clock style**: font (or the text font), weight Thin → Bold, separate sizes for Now Playing
  and World Clock; the clock and the date beside the player can each be switched off

## Taskbar monitor
The stats sit on a solid background in your taskbar's own colour (picked from the screen automatically, or *Transparent*), with adjustable **height** and **corner rounding**, so clicks land on them: **click** opens their settings, **drag** moves them.

Live **network ↑/↓, CPU, RAM, CPU speed, GPU, disk read/write, disk space, NPU** inside the
Windows taskbar, next to the tray (a port of TaskbarMonitor, same counters as Task Manager).
- Normal taskbar → two lines; small taskbar → one line (or force either); left/right
  taskbar → stacked icon-over-value cells (or icon + value rows on a wide one)
- Icons or text labels, text size, update interval, order (▲▼) and which items to show
- Even spacing between items (**Spacing** slider) and **per-item padding** — ⋯ next to an item opens Left / Top / Right / Bottom
- Drag it along the taskbar; right-click for settings / reset position / hide;
  double-click opens its settings
- Settings → Taskbar. (Close the old TaskbarMonitor app so you don't get two.)

### Alerts
Popups for **any Bluetooth device** connecting / disconnecting (mouse, keyboard, controller… with battery), **Wi-Fi / Bluetooth turned on or off**, **airplane mode**, **microphone muted / unmuted** — plus:
Volume · brightness (laptops) · Caps Lock (ON / OFF on the right) · keyboard language · charger / low battery ·
Wi-Fi & internet · headphones / Bluetooth audio connected (with battery %, also announced at
start-up) · song change · to-do reminders.

**Energy saver** on / off pops up too — when you switch it, and when Windows turns it on by itself at low battery
(Settings → Alerts → *Energy saver*).

When several popups arrive together (e.g. airplane mode turning Wi-Fi and Bluetooth off and a
headset disconnecting), each one shows for 0.2 s with a quick fade, and the **last** one stays
for the normal popup time.

- **Cover sizes**: album art on Now Playing and in the song-change popup (Settings → Pages)
- Keyboard backlight changes can't be shown: Windows has no way for apps to read them (laptop makers handle that key in their own firmware/software).
- Mice, keyboards and other non-audio Bluetooth devices can't be connected on demand by any
  app (Windows reconnects them by itself); clicking one opens Bluetooth settings. Work/school
  Wi-Fi (802.1X) opens Windows' network list for its sign-in.
- Notification *contents* can't be listed by a regular desktop app; the Controls page opens Windows' notification centre instead.

## Transparency (taskbar and File Explorer)
Settings → **Transparency**: a see-through taskbar and File Explorer, each with its own level (0–100 %), and a
look for what's behind (*Clear glass*: the desktop as it is · *Frosted glass*: blurred). Phemio puts a small
helper (`glass\mglass-<version>.dll` in the data folder, carried inside Phemio.exe — nothing to install)
into Explorer.
- **Taskbar** — the way TranslucentTB does it on Windows 11: through XAML diagnostics (the Windows feature
  behind Visual Studio's Live Visual Tree) the helper finds each taskbar's background rectangle and gives it
  the taskbar's own colour at your level; *Frosted glass* adds a Gaussian blur of what's behind (a composition
  effect, like TranslucentTB's blur brush). Explorer's own brush is kept and put back. The taskbar stats drop
  their own background meanwhile, so the taskbar's shows through them, in step.
- **File Explorer** — *What fades*:
  - *Background only* (the way the Windows 11 File Explorer Styler mod does it): the window's frame is extended
    over the whole window with a clear or blurred backdrop, and wherever File Explorer paints its dark-mode
    background colour it's painted see-through at your level instead — files, icons and text stay solid.
    Needs dark mode; otherwise it switches to whole window by itself.
  - *Whole window*: everything fades, never below 30 % visible.
- **Frosted glass** needs Windows' transparency effects on; Windows switches blur off while energy saver is on
  (then frosted looks like clear).
- If the taskbar's background can't be found (a Windows version whose taskbar is built differently), the page
  says *Not working on this Windows version* and the taskbar is left alone.
- **Undo.** *Restore Windows defaults* switches both off at once. Quitting Phemio (or a crash) also puts
  Windows' own look back; nothing is written into Windows. If Explorer restarts within a minute of switching
  this on, Phemio assumes it was the cause and switches transparency off by itself.
- Antivirus programs may warn about an app putting code into Explorer; that's this helper.
- **Helper log** (on the page) shows what the helper did, for when something doesn't work.

## Start with Windows
Settings → General → *Start with Windows* (on by default). The entry is written again every time Phemio
starts, so it follows the exe if you move it; if Task Manager's *Startup apps* (or a cleaner) switched it
off, it's switched back on — Phemio's own setting is what counts. At sign-in Phemio runs at high
priority for its first seconds (then normal again), and Windows' ~10 s hold on sign-in apps is turned off
for your user (`Explorer\Serialize\StartupDelayInMSec`), so the island shows as soon as the desktop does.
Started twice at sign-in, the second copy just quits.

0.33.5 fixes the reason it sometimes didn't start after a restart: right after Windows starts, Phemio
crashed while starting (a timer set "a minute ago" before the PC had been on a minute). If it ever happens,
`crash.log` in the data folder says why.

## Troubleshooting
- **Flicker**: 0.33.8 fixes the island flashing now and then: attached to the taskbar, it re-stacked its
  window inside the taskbar on every frame (which made the taskbar repaint); now only when one of
  Explorer's own windows really covers it. A new size or place (opening search from a bottom taskbar, a
  setting changed) no longer shows the old picture in the new place for a moment, and "Always on top" first
  tries a plain raise before the stronger re-stack.
- **Always on top**: with Settings → Island → *Always on top* on, the island checks every
  0.15 s that it's really the window in front at its position (and after any shell flyout
  opens or closes) and puts itself back on top if not.
- **Hide in full screen** only hides it for a window that covers the whole screen.
- **Something crashed?** `crash.log` in the data folder keeps the last few problems (version, time, which
  part). A problem inside Link no longer takes the whole app down — that part just stops and reconnects.
- **Something looks wrong** (say, a blank island)? Settings → About → **Copy diagnostics** copies the version,
  the island's state and the recent log — paste it into a message.
- `phemio.log` in the data folder (Settings → About → Open) notes each time the island was
  hidden for a full-screen app, shown again, or found covered — and by which program.

## Privacy
No accounts, no analytics. Tasks, shelf and settings live in `%APPDATA%\Phemio`.
Network use: Open-Meteo (only the city names you type — weather is always looked up by name, never by
this PC's location). Wi-Fi passwords you type go straight into Windows' own saved networks (a wrong
one is removed again); Phemio keeps no copy. Search stays on your PC (apps from Start, files from your local
Everything) — except web searches (`? …`, sent to Google), Gemini questions (`ai …`, sent to Google's Gemini API with your key), currency lookups (Google Finance, or open.er-api.com) and translations (`tr …`
and Alt+Q, sent to Google Translate or, if you pick it, Gemini; dictionary words, `dict …` / Alt+W, sent to Google Translate), and Link (only between your paired PCs on your local network, encrypted), which only go out when you ask for them. Nothing is saved. The content index (if you choose drives) keeps the text of your documents in
`%LOCALAPPDATA%\Phemio\contentindex.db` on this PC only; *Delete* in Settings empties it. A picture pasted after `<` is read by Gemini when you have a key (sent to Google's Gemini API), otherwise by Windows on this PC. The keyboard hook only reacts to
Caps Lock and the language-switch keys — and, for the drumming cat, to the fact that *a* key went down; nothing you type is read or stored.

## Memory
- Direct2D **software** target into a DIB + `UpdateLayeredWindow`: no Direct3D device, no GPU
  driver DLLs in the process.
- Redraws only while something animates; per-second timers run only while a ticking value is
  on screen (focus countdown, progress bar, clock pages).
- Media, brightness and network changes are pushed by Windows; web lookups run on short-lived
  threads; the settings window is destroyed when closed; working set trimmed after start-up.

Check it yourself in Settings → About (the same number Task Manager shows, plus working set and
committed memory) or in Task Manager.

//! Sound to the linked PC's speakers (Devices → Sound output → Play on
//! several → the other PC's row).
//!
//! The PC that plays the music records its main output (see `multiout`) and
//! sends it here as it is — no compression, a few megabits on the local
//! network — over a connection of its own, sealed like everything else in
//! Link. Silence travels as a count of frames, so nothing much is sent
//! while nothing plays, and the timeline stays whole.
//!
//! The PC that receives it plays it on its default output. It holds a
//! little sound back (`JITTER_MS`, plus the delay set on the first PC's
//! slider) so that late packets and the two PCs' slightly different clocks
//! don't show: the amount waiting is nudged back to that target by leaving
//! out, or repeating, one frame in a few thousand — inaudible — and only a
//! big jump (a stall on the network) is corrected at once.
//!
//! Only one way at a time: a PC that is receiving doesn't send its own
//! sound back (it would record what it is playing and send it round in a
//! loop). If both try at once, the PC with the smaller id sends.

use std::collections::VecDeque;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use super::proto::{self, Msg};

/// Held back on the playing PC before anything else (ms).
pub const JITTER_MS: usize = 50;
/// Of that, how much sits in the device's own buffer at most (ms).
const DEVICE_MS: usize = 30;

/// The id of the other PC's row in the output list.
pub fn target_id(peer: u64) -> String {
    format!("link:{peer:016x}")
}

pub fn is_remote(id: &str) -> bool {
    id.starts_with("link:")
}

/// The linked PC, as an output: (its row id, its name).
pub fn peer() -> Option<(String, String)> {
    super::with(|s| s.session.as_ref().map(|x| (target_id(x.id), x.name.clone()))).flatten()
}

// ---------------------------------------------------------------------------
// Sending (the PC that plays the music)
// ---------------------------------------------------------------------------

pub enum Out {
    Data(Vec<u8>),
    Gap(u32),
    Set { delay: u16, vol: f32 },
}

pub const CONNECTING: u8 = 0;
pub const PLAYING: u8 = 1;
pub const DEAD: u8 = 2;

/// The engine's end of the connection. Dropping it hangs up.
pub struct Tx {
    ch: SyncSender<Out>,
    state: Arc<AtomicU8>,
    /// Chunks left out because the network couldn't keep up.
    pub lost: u32,
}

impl Tx {
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }
    /// Queue something for the other PC; never waits (a stalled network
    /// loses sound rather than holding up the speakers here).
    pub fn send(&mut self, m: Out) {
        match self.ch.try_send(m) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.lost += 1,
            Err(TrySendError::Disconnected(_)) => self.state.store(DEAD, Ordering::Relaxed),
        }
    }
}

static SENDING: AtomicBool = AtomicBool::new(false);
/// What the row says under it: (state, a problem to show).
static STATUS: Mutex<(u8, String)> = Mutex::new((DEAD, String::new()));

fn set_status(state: u8, msg: &str) {
    if let Ok(mut s) = STATUS.lock() {
        *s = (state, msg.to_string());
    }
    super::tell_ui();
}

/// For the page: is the other PC playing our sound, and if not, why.
pub fn status() -> (u8, String) {
    STATUS.lock().map(|s| s.clone()).unwrap_or((DEAD, String::new()))
}

/// Start sending sound in this format (a WAVEFORMATEX, as bytes) to the
/// linked PC. Returns at once; the connection is made on a thread of its own.
pub fn open(fmt: Vec<u8>) -> Tx {
    let (ch, rx) = sync_channel::<Out>(256);
    let state = Arc::new(AtomicU8::new(CONNECTING));
    let st = state.clone();
    set_status(CONNECTING, "");
    super::spawn("link-audio-out", move || {
        SENDING.store(true, Ordering::SeqCst);
        let r = send_loop(fmt, rx, &st);
        SENDING.store(false, Ordering::SeqCst);
        st.store(DEAD, Ordering::Relaxed);
        match r {
            Ok(()) => set_status(DEAD, ""),
            Err(e) => {
                crate::util::log(&format!("link: sound to the other PC: {e}"));
                set_status(DEAD, &e.to_string());
            }
        }
    });
    Tx { ch, state, lost: 0 }
}

fn send_loop(fmt: Vec<u8>, rx: Receiver<Out>, state: &AtomicU8) -> std::io::Result<()> {
    let tr = crate::i18n::tr;
    if receiving() {
        return Err(proto::bad(tr("The other PC is playing its sound here")));
    }
    let (addr, peer) = super::session_addr().ok_or_else(|| proto::bad(tr("Not linked")))?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(4))?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(4)))?;
    s.set_write_timeout(Some(Duration::from_secs(3)))?;
    let (me, name) = super::me();
    let (id, _, mut tx, mut rx_c, _) = proto::handshake_for(&mut s, me, &name, None, super::secret_for, proto::FOR_AUDIO)?;
    if id != peer {
        return Err(proto::bad("another PC answered"));
    }
    // An older Phemio (or one with this switched off) hangs up here.
    match proto::read_sealed(&mut s, &mut rx_c) {
        Ok(Msg::AudioOk) => {}
        _ => return Err(proto::bad(tr("The other PC didn't take the sound (it needs Phemio 0.38 or newer, with Play sound from linked PCs on)"))),
    }
    proto::write_sealed(&mut s, &mut tx, &Msg::AudioFmt(fmt))?;
    state.store(PLAYING, Ordering::Relaxed);
    set_status(PLAYING, "");
    crate::util::log("link: sound goes to the other PC");
    while let Ok(m) = rx.recv() {
        let m = match m {
            Out::Data(b) => Msg::Audio(b),
            Out::Gap(frames) => Msg::AudioGap { frames },
            Out::Set { delay, vol } => Msg::AudioSet { delay, vol },
        };
        proto::write_sealed(&mut s, &mut tx, &m)?;
    }
    let _ = s.shutdown(std::net::Shutdown::Both);
    Ok(())
}

// ---------------------------------------------------------------------------
// Receiving (the PC whose speakers play)
// ---------------------------------------------------------------------------

/// The newest incoming stream (an older one sees this and ends).
static RX_GEN: AtomicU64 = AtomicU64::new(0);
static RECEIVING: AtomicBool = AtomicBool::new(false);

/// Is the other PC playing its sound here right now?
pub fn receiving() -> bool {
    RECEIVING.load(Ordering::SeqCst)
}

struct Shared {
    queue: VecDeque<u8>,
    delay: u16,
    vol: f32,
    closed: bool,
}

/// A WAVEFORMATEX we'd hand to Windows: sizes that add up, sane numbers.
fn format_ok(f: &[u8]) -> Option<(usize, usize)> {
    if f.len() < 18 || f.len() > 64 {
        return None;
    }
    let u16_at = |i: usize| u16::from_le_bytes([f[i], f[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes([f[i], f[i + 1], f[i + 2], f[i + 3]]) as usize;
    let (tag, ch, rate, avg, block, bits, cb) = (u16_at(0), u16_at(2), u32_at(4), u32_at(8), u16_at(12), u16_at(14), u16_at(16));
    let ok = (tag == 1 || tag == 3 || (tag == 0xFFFE && cb == 22))
        && (tag == 0xFFFE || cb == 0)
        && f.len() == 18 + cb
        && (1..=8).contains(&ch)
        && (8000..=384_000).contains(&rate)
        && matches!(bits, 16 | 24 | 32)
        && block == ch * bits / 8
        && avg == rate * block;
    ok.then_some((rate, block))
}

/// The other PC called to play its sound here (from `listen`).
pub fn receive(mut s: TcpStream, mut tx: proto::Cipher, mut rx: proto::Cipher, peer: u64) -> std::io::Result<()> {
    if !crate::config::cur().b("link_audio_in") {
        return Err(proto::bad("playing sound from linked PCs is switched off here"));
    }
    // Both PCs sending to each other would go round in a loop: the smaller id sends.
    if SENDING.load(Ordering::SeqCst) && super::me().0 < peer {
        return Err(proto::bad("already sending sound the other way"));
    }
    proto::write_sealed(&mut s, &mut tx, &Msg::AudioOk)?;
    // (Silence is sent as counts, several a second: quiet for this long means gone.)
    s.set_read_timeout(Some(Duration::from_secs(8)))?;
    let Msg::AudioFmt(fmt) = proto::read_sealed(&mut s, &mut rx)? else { return Err(proto::bad("expected the sound format")) };
    let (rate, block) = format_ok(&fmt).ok_or_else(|| proto::bad("a sound format this PC can't play"))?;
    let gen = RX_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let sh = Arc::new(Mutex::new(Shared { queue: VecDeque::new(), delay: 0, vol: 1.0, closed: false }));
    RECEIVING.store(true, Ordering::SeqCst);
    crate::util::log(&format!("link: playing the other PC's sound here ({rate} Hz, {block} B/frame)"));
    super::tell_ui();
    {
        let sh = sh.clone();
        super::spawn("link-audio-play", move || player(sh, fmt, rate, block, gen));
    }
    // At most this much waiting (a stalled player must not eat memory).
    let cap = rate * block * 3;
    let r = loop {
        if RX_GEN.load(Ordering::SeqCst) != gen || !super::running() {
            break Ok(());
        }
        let m = match proto::read_sealed(&mut s, &mut rx) {
            Ok(m) => m,
            Err(e) => break Err(e),
        };
        let Ok(mut g) = sh.lock() else { break Ok(()) };
        match m {
            Msg::Audio(b) => {
                let n = b.len() / block * block;
                g.queue.extend(&b[..n]);
            }
            Msg::AudioGap { frames } => {
                let n = (frames as usize).min(rate * 2) * block;
                g.queue.extend(std::iter::repeat_n(0u8, n));
            }
            Msg::AudioSet { delay, vol } => {
                g.delay = delay.min(crate::multiout::MAX_DELAY as u16 + 1000);
                g.vol = if vol.is_finite() { vol.clamp(0.0, 1.0) } else { 1.0 };
            }
            _ => {}
        }
        if g.queue.len() > cap {
            let cut = (g.queue.len() - cap / 2) / block * block;
            g.queue.drain(..cut);
        }
    };
    if let Ok(mut g) = sh.lock() {
        g.closed = true;
    }
    if RX_GEN.load(Ordering::SeqCst) == gen {
        RECEIVING.store(false, Ordering::SeqCst);
    }
    crate::util::log("link: the other PC's sound stopped");
    super::tell_ui();
    // (The sender hanging up is the normal end.)
    match r {
        Err(e) if e.kind() != std::io::ErrorKind::UnexpectedEof && e.kind() != std::io::ErrorKind::ConnectionReset && e.kind() != std::io::ErrorKind::ConnectionAborted => Err(e),
        _ => Ok(()),
    }
}

/// Plays what `receive` queues on this PC's default output, until the stream
/// closes. A device that goes away (unplugged, another one made default) is
/// opened again.
fn player(sh: Arc<Mutex<Shared>>, fmt: Vec<u8>, rate: usize, block: usize, gen: u64) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let over = || RX_GEN.load(Ordering::SeqCst) != gen || sh.lock().map(|g| g.closed).unwrap_or(true);
    while !over() {
        if let Err(e) = unsafe { play(&sh, &fmt, rate, block, &over) } {
            crate::util::log(&format!("link: playing the other PC's sound: {}", e.message()));
            std::thread::sleep(Duration::from_millis(700));
        }
    }
    unsafe { CoUninitialize() };
}

/// What to do with the next frames: leave one out, or play one twice, every `every` frames.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Trim {
    None,
    Drop(u32),
    Repeat(u32),
}

/// How to steer from the smoothed error (frames waiting − wanted). Nothing
/// inside ±6 ms (what's waiting wobbles by a few ms as packets arrive); once
/// steering, keep on until it's back within 1.5 ms (`was`: steering now).
fn trim_for(err: f64, rate: usize, was: Trim) -> Trim {
    let ms = |m: f64| m * rate as f64 / 1000.0;
    let start = if was == Trim::None { 6.0 } else { 1.5 };
    // One frame in 2048 moves it half a millisecond a second (a clock that
    // runs a little fast or slow needs less); further out, quicker.
    let every = |e: f64| if e > ms(15.0) { 256 } else if e > ms(6.0) { 512 } else { 2048 };
    if err > ms(start) && !matches!(was, Trim::Repeat(_)) {
        Trim::Drop(every(err))
    } else if err < -ms(start) && !matches!(was, Trim::Drop(_)) {
        Trim::Repeat(every(-err))
    } else {
        Trim::None
    }
}

/// Take up to `n` frames for the device out of `q`, trimmed as asked.
/// `ctr` counts frames across calls. Returns the bytes to play.
fn take(q: &mut VecDeque<u8>, n: usize, block: usize, trim: Trim, ctr: &mut u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(n * block);
    let mut frame = vec![0u8; block];
    while out.len() < n * block && q.len() >= block {
        for (d, s) in frame.iter_mut().zip(q.drain(..block)) {
            *d = s;
        }
        *ctr = ctr.wrapping_add(1);
        match trim {
            Trim::Drop(every) if *ctr % every == 0 => continue,
            Trim::Repeat(every) if *ctr % every == 0 && out.len() + 2 * block <= n * block => {
                out.extend_from_slice(&frame);
                out.extend_from_slice(&frame);
            }
            _ => out.extend_from_slice(&frame),
        }
    }
    out
}

unsafe fn play(sh: &Mutex<Shared>, fmt: &[u8], rate: usize, block: usize, over: &dyn Fn() -> bool) -> windows::core::Result<()> {
    let en = crate::audio::enumerator().ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    let dev = en.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    // Their format; Windows converts to what this device wants.
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 2_000_000, 0, fmt.as_ptr() as *const WAVEFORMATEX, None)?;
    let render: IAudioRenderClient = client.GetService()?;
    let volume: Option<ISimpleAudioVolume> = client.GetService().ok();
    let buf = client.GetBufferSize()? as usize;
    client.Start()?;
    let ms = |m: usize| m * rate / 1000;
    let mut playing = false;
    let mut smooth = 0f64;
    let mut ctr = 0u32;
    let mut vol_set = -1f32;
    let mut trim = Trim::None;
    let mut last = Instant::now();
    // (Developer aid, ISLET_AUDIO_LOG=1: how much is waiting, every 2 s.)
    let chatty = std::env::var("ISLET_AUDIO_LOG").is_ok();
    let mut said = Instant::now();
    let r = loop {
        if over() {
            break Ok(());
        }
        let pad = match client.GetCurrentPadding() {
            Ok(p) => p as usize,
            Err(e) => break Err(e),
        };
        let chunk = {
            let Ok(mut g) = sh.lock() else { break Ok(()) };
            if (g.vol - vol_set).abs() > 0.001 {
                if let Some(v) = &volume {
                    let _ = v.SetMasterVolume(g.vol, std::ptr::null());
                }
                vol_set = g.vol;
            }
            let want = ms(JITTER_MS + g.delay as usize);
            let waiting = g.queue.len() / block + pad;
            if !playing {
                // Starting (or starting again after running dry): hold back
                // the wanted amount first.
                if g.queue.len() < block {
                    None
                } else {
                    if waiting < want {
                        for _ in 0..(want - waiting) * block {
                            g.queue.push_front(0);
                        }
                    }
                    playing = true;
                    smooth = want as f64;
                    last = Instant::now();
                    Some(Vec::new())
                }
            } else if g.queue.len() < block && pad == 0 {
                playing = false;
                None
            } else {
                // Smoothed over about half a second (packets come in bursts).
                let dt = last.elapsed().as_secs_f64().min(0.1);
                last = Instant::now();
                smooth += (waiting as f64 - smooth) * (1.0 - (-dt / 0.5).exp());
                // A big jump (the delay was changed, the network stalled): at once.
                if waiting > want + ms(80) {
                    let cut = ((waiting - want) * block).min(g.queue.len() / block * block);
                    g.queue.drain(..cut);
                    smooth = want as f64;
                } else if smooth < want as f64 - ms(40) as f64 {
                    for _ in 0..(want - waiting.min(want)) * block {
                        g.queue.push_front(0);
                    }
                    smooth = want as f64;
                }
                let room = buf.saturating_sub(pad).min(ms(DEVICE_MS).saturating_sub(pad));
                if chatty && said.elapsed() >= Duration::from_secs(2) {
                    said = Instant::now();
                    let to_ms = |f: f64| f * 1000.0 / rate as f64;
                    crate::util::log(&format!("link: sound here: {:.0} ms waiting ({:.0} in the device), {:.1} smoothed, {:.0} wanted", to_ms(waiting as f64), to_ms(pad as f64), to_ms(smooth), to_ms(want as f64)));
                }
                trim = trim_for(smooth - want as f64, rate, trim);
                Some(take(&mut g.queue, room, block, trim, &mut ctr))
            }
        };
        if let Some(c) = chunk.filter(|c| !c.is_empty()) {
            let n = c.len() / block;
            match render.GetBuffer(n as u32) {
                Ok(p) => {
                    std::ptr::copy_nonoverlapping(c.as_ptr(), p, n * block);
                    let _ = render.ReleaseBuffer(n as u32, 0);
                }
                Err(e) => break Err(e),
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    };
    let _ = client.Stop();
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(n: usize) -> VecDeque<u8> {
        // 2-byte frames numbered 0, 1, 2 …
        (0..n as u16).flat_map(|i| i.to_le_bytes()).collect()
    }

    #[test]
    fn take_plain_drop_and_repeat() {
        let mut ctr = 0;
        let mut q = frames(100);
        assert_eq!(take(&mut q, 10, 2, Trim::None, &mut ctr).len(), 20);
        assert_eq!(q.len(), 180);
        // Leaving one out in four: 11 taken (two of them left out), 9 played.
        let (mut q, mut ctr) = (frames(100), 0);
        let out = take(&mut q, 9, 2, Trim::Drop(4), &mut ctr);
        assert_eq!(out.len(), 18);
        assert_eq!(q.len(), 200 - 22);
        assert!(!out.chunks(2).any(|f| f == 7u16.to_le_bytes()));
        assert!(!out.chunks(2).any(|f| f == 3u16.to_le_bytes()));
        // Playing one in four twice: 8 taken, 10 played.
        let (mut q, mut ctr) = (frames(100), 0);
        let out = take(&mut q, 10, 2, Trim::Repeat(4), &mut ctr);
        assert_eq!(out.len(), 20);
        assert_eq!(q.len(), 200 - 16);
        // Never more than asked, never from an empty queue.
        let (mut q, mut ctr) = (frames(3), 0);
        assert_eq!(take(&mut q, 10, 2, Trim::Repeat(2), &mut ctr).len(), 8);
        assert!(q.is_empty());
    }

    #[test]
    fn steering() {
        let n = Trim::None;
        assert!(trim_for(0.0, 48000, n) == n);
        // Inside the wobble: nothing; outside: gently; far out: faster.
        assert!(trim_for(48.0 * 5.0, 48000, n) == n);
        assert!(trim_for(48.0 * 8.0, 48000, n) == Trim::Drop(512));
        assert!(trim_for(48.0 * 20.0, 48000, n) == Trim::Drop(256));
        assert!(trim_for(-48.0 * 8.0, 48000, n) == Trim::Repeat(512));
        assert!(trim_for(-48.0 * 20.0, 48000, n) == Trim::Repeat(256));
        // Once steering, on until nearly there — and never past it the other way.
        assert!(trim_for(48.0 * 3.0, 48000, Trim::Drop(2048)) == Trim::Drop(2048));
        assert!(trim_for(48.0 * 1.0, 48000, Trim::Drop(2048)) == n);
        assert!(trim_for(-48.0 * 3.0, 48000, Trim::Drop(2048)) == n);
        assert!(trim_for(-48.0 * 3.0, 48000, Trim::Repeat(2048)) == Trim::Repeat(2048));
    }

    #[test]
    fn formats() {
        // 48 kHz stereo float, extensible (what Windows mixes in).
        let mut f = Vec::new();
        f.extend(0xFFFEu16.to_le_bytes());
        f.extend(2u16.to_le_bytes());
        f.extend(48000u32.to_le_bytes());
        f.extend((48000u32 * 8).to_le_bytes());
        f.extend(8u16.to_le_bytes());
        f.extend(32u16.to_le_bytes());
        f.extend(22u16.to_le_bytes());
        f.extend([0u8; 22]);
        assert_eq!(format_ok(&f), Some((48000, 8)));
        // A block size that doesn't match would overrun the buffer.
        let mut bad = f.clone();
        bad[12] = 4;
        assert_eq!(format_ok(&bad), None);
        assert_eq!(format_ok(&f[..30]), None);
        assert_eq!(format_ok(&[]), None);
    }

    #[test]
    fn ids() {
        assert!(is_remote(&target_id(7)));
        assert!(!is_remote("{0.0.0.00000000}.{abc}"));
    }
}

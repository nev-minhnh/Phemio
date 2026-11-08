//! Now-playing info for any player that shows up in the Windows media overlay
//! (Spotify, YouTube in a browser, Apple Music, …) via
//! GlobalSystemMediaTransportControlsSessionManager.
//!
//! All WinRT work happens on one background MTA thread that sleeps on a
//! channel; Windows pushes change events, so nothing is polled.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Storage::Streams::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Default)]
pub struct MediaInfo {
    pub has_session: bool,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Encoded thumbnail (PNG/JPEG) – decoded on the UI thread.
    pub art: Option<Arc<Vec<u8>>>,
    /// Changes whenever the thumbnail changes.
    pub art_id: u64,
    pub pos_ms: i64,
    pub dur_ms: i64,
    /// FILETIME (100 ns) at which `pos_ms` was sampled.
    pub pos_stamp: i64,
    /// The playing app (its AppUserModelID: "Spotify.exe",
    /// "SpotifyAB.SpotifyMusic_…!Spotify", "Chrome._crx_…" for a web app, "MSEdge"…).
    pub app_id: String,
    /// Repeat: 0 off, 1 this song, 2 the whole list. `repeat_native`: the
    /// player does it itself (else Phemio repeats one song for it).
    pub repeat: u8,
    pub repeat_native: bool,
}

/// Now Playing's "stop after this song": when a song ends, the next one
/// doesn't play (it's paused at its start).
static STOP_AFTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn stop_after() -> bool {
    STOP_AFTER.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_stop_after(on: bool) {
    STOP_AFTER.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Repeat one song for players that can't do it themselves (0 off, 1 on).
/// Stop after this song: the latest end-of-song check scheduled.
static STOP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static EMU_REPEAT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

impl MediaInfo {
    /// Current playback position, extrapolated while playing.
    pub fn position_ms(&self) -> i64 {
        if self.playing && self.pos_stamp > 0 {
            let p = self.pos_ms + (filetime_now() - self.pos_stamp) / 10_000;
            p.clamp(0, self.dur_ms.max(0))
        } else {
            self.pos_ms
        }
    }
}

pub fn filetime_now() -> i64 {
    unsafe {
        let ft = GetSystemTimeAsFileTime();
        ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
    }
}

static SHARED: Mutex<Option<MediaInfo>> = Mutex::new(None);

pub fn snapshot() -> MediaInfo {
    let real = SHARED.lock().ok().and_then(|g| g.clone()).unwrap_or_default();
    if !real.has_session {
        if let Some(d) = demo() {
            return d;
        }
    }
    real
}

/// Developer aid: ISLET_DEMO_ART=<image file> fakes a playing track so the UI
/// can be checked on machines without a media session.
fn demo() -> Option<MediaInfo> {
    static ART: std::sync::OnceLock<Option<Arc<Vec<u8>>>> = std::sync::OnceLock::new();
    let art = ART.get_or_init(|| std::env::var("ISLET_DEMO_ART").ok().and_then(|p| std::fs::read(p).ok()).map(Arc::new)).clone()?;
    Some(MediaInfo {
        has_session: true,
        title: "Dirtmouth".into(),
        artist: "Christopher Larkin".into(),
        playing: true,
        art: Some(art),
        art_id: u64::MAX,
        pos_ms: 62_000,
        dur_ms: 193_000,
        pos_stamp: filetime_now(),
        app_id: std::env::var("ISLET_DEMO_APP").unwrap_or_else(|_| "Spotify.exe".into()),
        ..Default::default()
    })
}

pub enum Cmd {
    Seek(i64),
    Toggle,
    Next,
    Prev,
    /// Repeat: 0 off, 1 this song, 2 the whole list.
    Repeat(u8),
    /// Stop after this song: its end is due now (the check's number).
    StopAtEnd(u64),
    Refresh,
    Quit,
}

pub struct Media {
    tx: Sender<Cmd>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Media {
    pub fn start(notify: HWND, msg: u32) -> Media {
        let (tx, rx) = channel();
        let tx2 = tx.clone();
        let hwnd = notify.0 as isize;
        let thread = std::thread::Builder::new()
            .name("media".into())
            .stack_size(256 * 1024)
            .spawn(move || unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let _ = worker(rx, tx2, hwnd, msg);
                CoUninitialize();
            })
            .ok();
        if demo().is_some() {
            unsafe {
                let _ = PostMessageW(Some(notify), msg, WPARAM(0), LPARAM(0));
            }
        }
        Media { tx, thread }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut g) = SHARED.lock() {
            *g = None;
        }
    }
}

struct Subs {
    session: GlobalSystemMediaTransportControlsSession,
    t_props: i64,
    t_play: i64,
    t_time: i64,
}

fn worker(rx: Receiver<Cmd>, tx: Sender<Cmd>, hwnd: isize, msg: u32) -> Result<()> {
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
    let t = tx.clone();
    let tok_mgr = mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    let t = tx.clone();
    let tok_sessions = mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;

    let mut subs: Option<Subs> = None;
    let mut last_art_key = String::new();
    let mut art_id = 0u64;
    let mut art: Option<Arc<Vec<u8>>> = None;
    let mut first = true;
    // After a new song, look at its picture again for a few seconds: browsers
    // (YouTube Music in Chrome / Edge / Brave) first publish the site's icon
    // and swap in the real cover a moment later, with the same title.
    let mut art_hash = 0u64;
    let mut recheck_until: Option<std::time::Instant> = None;
    // For repeating one song when the player can't: where the last song had
    // got to, and when you last skipped yourself.
    let mut last_song: Option<(String, i64, i64, i64)> = None;
    let mut skipped_at: Option<std::time::Instant> = None;

    loop {
        let cmd = if first {
            first = false;
            Cmd::Refresh
        } else {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        };
        match cmd {
            Cmd::Quit => break,
            Cmd::Repeat(mode) => {
                if let Ok(s) = mgr.GetCurrentSession() {
                    let native = s.GetPlaybackInfo().and_then(|p| p.Controls()).and_then(|c| c.IsRepeatEnabled()).unwrap_or(false);
                    if native {
                        EMU_REPEAT.store(0, std::sync::atomic::Ordering::Relaxed);
                        if let Ok(op) = s.TryChangeAutoRepeatModeAsync(windows::Media::MediaPlaybackAutoRepeatMode(mode as i32)) {
                            let _ = op.join();
                        }
                    } else {
                        EMU_REPEAT.store((mode != 0) as u8, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                let _ = tx.send(Cmd::Refresh);
                continue;
            }
            Cmd::StopAtEnd(gen) => {
                // The song is about to end: stop it right there (repeat or
                // not), back at its start — play plays it again.
                if gen == STOP_GEN.load(std::sync::atomic::Ordering::SeqCst) && STOP_AFTER.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(s) = mgr.GetCurrentSession() {
                        let playing = s.GetPlaybackInfo().and_then(|p| p.PlaybackStatus()).map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing).unwrap_or(false);
                        let left = s.GetTimelineProperties().ok().map(|tl| {
                            let pos = tl.Position().map(|t| t.Duration).unwrap_or(0);
                            let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
                            let stamp = tl.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
                            let since = if stamp > 0 { (filetime_now() - stamp).clamp(0, 10_000_000 * 3600) } else { 0 };
                            (end - pos - since) / 10_000
                        });
                        if playing && left.map(|l| l <= 900).unwrap_or(false) {
                            if let Ok(op) = s.TryPauseAsync() {
                                let _ = op.join();
                            }
                            if let Ok(op) = s.TryChangePlaybackPositionAsync(0) {
                                let _ = op.join();
                            }
                            skipped_at = Some(std::time::Instant::now());
                        }
                    }
                }
                let _ = tx.send(Cmd::Refresh);
                continue;
            }
            Cmd::Seek(_) | Cmd::Toggle | Cmd::Next | Cmd::Prev => {
                if matches!(cmd, Cmd::Next | Cmd::Prev) {
                    skipped_at = Some(std::time::Instant::now());
                }
                if let Ok(s) = mgr.GetCurrentSession() {
                    let r = match cmd {
                        Cmd::Seek(ms) => s.TryChangePlaybackPositionAsync(ms * 10_000),
                        Cmd::Toggle => s.TryTogglePlayPauseAsync(),
                        Cmd::Next => s.TrySkipNextAsync(),
                        _ => s.TrySkipPreviousAsync(),
                    };
                    if let Ok(op) = r {
                        let _ = op.join();
                    }
                }
                continue;
            }
            Cmd::Refresh => {}
        }
        // Coalesce bursts of events (players fire several at once).
        loop {
            match rx.recv_timeout(Duration::from_millis(60)) {
                Ok(Cmd::Quit) => return cleanup(&mgr, tok_mgr, tok_sessions, subs),
                Ok(Cmd::Refresh) => continue,
                Ok(other) => {
                    let _ = tx.send(other);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return cleanup(&mgr, tok_mgr, tok_sessions, subs),
            }
        }

        let session = mgr.GetCurrentSession().ok();
        // (Re)subscribe when the active session changes.
        let same = match (&subs, &session) {
            (Some(s), Some(n)) => s.session == *n,
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some(s) = subs.take() {
                unsub(&s);
            }
            if let Some(n) = &session {
                subs = subscribe(n, &tx).ok();
            }
        }

        let mut info = MediaInfo::default();
        if let Some(s) = &session {
            info.has_session = true;
            info.app_id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            if let Ok(p) = s.TryGetMediaPropertiesAsync().and_then(|op| op.join()) {
                info.title = p.Title().map(|h| h.to_string()).unwrap_or_default();
                info.artist = p.Artist().map(|h| h.to_string()).unwrap_or_default();
                if info.artist.is_empty() {
                    info.artist = p.AlbumArtist().map(|h| h.to_string()).unwrap_or_default();
                }
                let key = format!("{}\u{1}{}", info.title, info.artist);
                let fresh = key != last_art_key;
                let rechecking = recheck_until.map(|t| std::time::Instant::now() < t).unwrap_or(false);
                if fresh || art.is_none() || rechecking || PROPS_CHANGED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    if fresh {
                        last_art_key = key;
                        recheck_until = Some(std::time::Instant::now() + Duration::from_secs(8));
                        // Ask again shortly, in case no event says the cover arrived.
                        for ms in [700u64, 2000, 5000] {
                            let t = tx.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(Duration::from_millis(ms));
                                let _ = t.send(Cmd::Refresh);
                            });
                        }
                    }
                    let b = p.Thumbnail().ok().and_then(|r| read_thumb(&r).ok());
                    let h = b.as_ref().map(|b| hash_bytes(b)).unwrap_or(0);
                    // A new picture (or the song changed): show it.
                    if fresh || h != art_hash {
                        art_hash = h;
                        art = b.map(Arc::new);
                        art_id += 1;
                    }
                }
            }
            if let Ok(pi) = s.GetPlaybackInfo() {
                info.playing = pi.PlaybackStatus().map(|st| st == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing).unwrap_or(false);
                info.repeat_native = pi.Controls().and_then(|c| c.IsRepeatEnabled()).unwrap_or(false);
                info.repeat = if info.repeat_native {
                    pi.AutoRepeatMode().and_then(|r| r.Value()).map(|m| m.0.clamp(0, 2) as u8).unwrap_or(0)
                } else {
                    EMU_REPEAT.load(std::sync::atomic::Ordering::Relaxed)
                };
            }
            if let Ok(tl) = s.GetTimelineProperties() {
                let pos = tl.Position().map(|t| t.Duration).unwrap_or(0);
                let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
                let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
                info.pos_ms = (pos - start) / 10_000;
                info.dur_ms = (end - start) / 10_000;
                info.pos_stamp = tl.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
                if info.pos_stamp <= 0 || filetime_now() - info.pos_stamp > 10_000_000 * 3600 {
                    info.pos_stamp = filetime_now();
                }
            }
            info.art = art.clone();
            info.art_id = art_id;
            // Repeat one song for a player that can't: the song changed by
            // itself (it had played to its end, you didn't skip) — back to it.
            let key = format!("{}\u{1}{}", info.title, info.artist);
            // Did the song change by itself (the last one played to its end,
            // not skipped by you)?
            let ended = !info.title.is_empty()
                && last_song.as_ref().map(|(prev_key, pos, dur, stamp)| {
                    let reached = pos + (filetime_now() - stamp).max(0) / 10_000;
                    let by_you = skipped_at.map(|t| t.elapsed() < Duration::from_secs(4)).unwrap_or(false);
                    *prev_key != key && *dur > 5_000 && !by_you && reached >= dur - 4_000
                }).unwrap_or(false);
            if ended && !info.repeat_native && info.repeat == 1 {
                if let Ok(op) = s.TrySkipPreviousAsync() {
                    let _ = op.join();
                }
                skipped_at = Some(std::time::Instant::now());
            } else if ended && info.repeat != 1 && STOP_AFTER.load(std::sync::atomic::Ordering::Relaxed) {
                // Stop after that song: back to it, paused at its start
                // (play replays it); the next song doesn't start.
                if let Ok(op) = s.TryPauseAsync() {
                    let _ = op.join();
                }
                if let Ok(op) = s.TrySkipPreviousAsync() {
                    let _ = op.join();
                }
                skipped_at = Some(std::time::Instant::now());
                std::thread::sleep(Duration::from_millis(250));
                if let Ok(op) = s.TryPauseAsync() {
                    let _ = op.join();
                }
                if let Ok(op) = s.TryChangePlaybackPositionAsync(0) {
                    let _ = op.join();
                }
                info.playing = false;
                info.pos_ms = 0;
            }
            // Stop after this song: a check timed for just before its end.
            if STOP_AFTER.load(std::sync::atomic::Ordering::Relaxed) && info.playing && info.dur_ms > 3_000 {
                let since = ((filetime_now() - info.pos_stamp).max(0) / 10_000).min(3_600_000);
                let left = info.dur_ms - info.pos_ms - since;
                let gen = STOP_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let t = tx.clone();
                // (Fired a little early: the pause has to land before the
                // player moves on. Long waits are re-checked on the way.)
                let wait = (left - 250).clamp(0, 30_000) as u64;
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(wait));
                    if STOP_GEN.load(std::sync::atomic::Ordering::SeqCst) == gen {
                        let _ = t.send(if wait >= 30_000 { Cmd::Refresh } else { Cmd::StopAtEnd(gen) });
                    }
                });
            } else {
                STOP_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            if !info.title.is_empty() {
                let stamp = if info.playing { info.pos_stamp } else { filetime_now() };
                last_song = Some((key, info.pos_ms, info.dur_ms, stamp));
            }
        } else {
            last_art_key.clear();
            art = None;
            art_hash = 0;
        }
        if let Ok(mut g) = SHARED.lock() {
            *g = Some(info);
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
    cleanup(&mgr, tok_mgr, tok_sessions, subs)
}

fn cleanup(mgr: &GlobalSystemMediaTransportControlsSessionManager, a: i64, b: i64, subs: Option<Subs>) -> Result<()> {
    let _ = mgr.RemoveCurrentSessionChanged(a);
    let _ = mgr.RemoveSessionsChanged(b);
    if let Some(s) = subs {
        unsub(&s);
    }
    Ok(())
}

fn subscribe(s: &GlobalSystemMediaTransportControlsSession, tx: &Sender<Cmd>) -> Result<Subs> {
    let t = tx.clone();
    let t_props = s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
        PROPS_CHANGED.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    let t = tx.clone();
    let t_play = s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    let t = tx.clone();
    let t_time = s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Cmd::Refresh);
        Ok(())
    }))?;
    Ok(Subs { session: s.clone(), t_props, t_play, t_time })
}

fn unsub(s: &Subs) {
    let _ = s.session.RemoveMediaPropertiesChanged(s.t_props);
    let _ = s.session.RemovePlaybackInfoChanged(s.t_play);
    let _ = s.session.RemoveTimelinePropertiesChanged(s.t_time);
}

/// The song's details changed (its cover may have come in).
static PROPS_CHANGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn hash_bytes(b: &[u8]) -> u64 {
    // FNV-1a.
    let mut h = 0xcbf29ce484222325u64;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn read_thumb(r: &IRandomAccessStreamReference) -> Result<Vec<u8>> {
    let stream = r.OpenReadAsync()?.join()?;
    let size = stream.Size()? as u32;
    if size == 0 || size > 8 * 1024 * 1024 {
        return Err(E_FAIL.into());
    }
    let input: IInputStream = stream.cast()?;
    let reader = DataReader::CreateDataReader(&input)?;
    reader.LoadAsync(size)?.join()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf)?;
    let _ = reader.DetachStream();
    Ok(buf)
}

//! What the two PCs say to each other, and how it's kept private.
//!
//! Frames on the TCP connection: `u32 length` + `u8 kind` + payload, where
//! kind 0 is a plain message (only the greetings before a session) and kind 1
//! is AES-256-GCM sealed (everything else).
//!
//! Pairing: both sides make a fresh X25519 key and swap the public halves.
//! From the shared secret each side works out the same 6-digit code; you
//! check the two screens show the same number and confirm on both. A
//! long-term secret for the pair comes from the same exchange.
//!
//! Every later connection: both sides send a random nonce; the pair secret
//! and both nonces give one key per direction. A message counter is the
//! nonce for each frame, so nothing can be replayed or reordered, and a PC
//! without the secret can't read or send anything.
//!
//! No Windows calls here: it's tested on its own (also over a real socket).

use std::io::{Read, Write};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha256};

pub const PROTO: u16 = 3;

/// What a connection is for (sent in the greeting).
pub const FOR_SESSION: u8 = 0;
pub const FOR_FILES: u8 = 1;

/// What a batch of files is for.
pub const BATCH_CLIPBOARD: u8 = 0;
pub const BATCH_IMAGE: u8 = 1;
pub const BATCH_SHELF: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct ScreenInfo {
    pub key: String,
    pub label: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    // ---- In the clear (before a session) ----
    Hello { proto: u16, id: u64, name: String, nonce: [u8; 16], purpose: u8 },
    PairReq { proto: u16, id: u64, name: String, pk: [u8; 32] },
    PairResp { id: u64, name: String, pk: [u8; 32] },
    PairConfirm { ok: bool },
    // ---- Sealed ----
    Ready,
    Screens(Vec<ScreenInfo>),
    /// Where every screen sits on the shared map; the newest version wins.
    Layout { version: u64, places: Vec<(String, f32, f32)> },
    /// The pointer arrives on your screen at this pixel (your desktop).
    Enter { x: i32, y: i32 },
    Move { x: i32, y: i32 },
    /// The pointer went back: let go of anything held.
    Leave,
    /// 0 left, 1 right, 2 middle, 3 back, 4 forward.
    Button { b: u8, down: bool },
    Wheel { delta: i16, horizontal: bool },
    /// Virtual key, scan code, flags (1 extended, 2 up).
    Key { vk: u16, scan: u16, flags: u8 },
    /// Your mouse moved over there: you have it back.
    Take,
    Ping { t: u64 },
    Pong { t: u64 },
    Bye,
    // ---- Sharing (session) ----
    /// Text you copied over there.
    Clip { text: String },
    /// Show the screens' numbers for a few seconds.
    Identify,
    /// Make this display (its device name) your main screen.
    MainScreen { key: String },
    /// What's this display of yours set to? (answered with DispInfo)
    DispAsk { key: String },
    /// A display's settings: scale now / recommended / on offer, resolution
    /// now / recommended / on offer (landscape), orientation, and seconds
    /// left before a change goes back (0: none waiting).
    DispInfo { key: String, scale: i32, scale_rec: i32, scales: Vec<i32>, res: (i32, i32), res_rec: (i32, i32), modes: Vec<(i32, i32)>, orient: i32, pending: i32 },
    /// Change a display of yours: op 1 scale (a), 2 resolution (a × b) and
    /// orientation (c), 3 keep the change, 4 go back.
    DispSet { key: String, op: u8, a: i32, b: i32, c: i32 },
    /// What's on your shelf: (id, name, size, is a folder).
    ShelfList(Vec<(u64, String, u64, bool)>),
    /// Send me these shelf items.
    ShelfGet(Vec<u64>),
    /// What's in each shelf item (id, content hash), worked out after the list.
    ShelfHashes(Vec<(u64, u64)>),
    // ---- Files (their own connection, so the mouse never waits) ----
    Batch { id: u64, kind: u8, count: u32, total: u64 },
    /// The next file (path inside the batch, size) or folder.
    FileHead { rel: String, size: u64, dir: bool },
    Data(Vec<u8>),
    BatchEnd,
    /// The other side has it all (or not).
    BatchAck { ok: bool },
}

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn str(&mut self, s: &str) {
        // Cut on a character boundary.
        let mut n = s.len().min(4000);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        let b = &s.as_bytes()[..n];
        self.u16(b.len() as u16);
        self.bytes(b);
    }
    fn blob(&mut self, b: &[u8]) {
        self.0.extend_from_slice(&(b.len() as u32).to_le_bytes());
        self.bytes(b);
    }
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.0.get(self.1..self.1 + n)?;
        self.1 += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        self.take(2).map(|b| i16::from_le_bytes([b[0], b[1]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.take(4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn f32(&mut self) -> Option<f32> {
        self.take(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()))
    }
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N).map(|b| b.try_into().unwrap())
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u16()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn blob(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n).map(|b| b.to_vec())
    }
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(16));
        match self {
            Msg::Hello { proto, id, name, nonce, purpose } => {
                w.u8(1);
                w.u16(*proto);
                w.u64(*id);
                w.str(name);
                w.bytes(nonce);
                w.u8(*purpose);
            }
            Msg::PairReq { proto, id, name, pk } => {
                w.u8(2);
                w.u16(*proto);
                w.u64(*id);
                w.str(name);
                w.bytes(pk);
            }
            Msg::PairResp { id, name, pk } => {
                w.u8(3);
                w.u64(*id);
                w.str(name);
                w.bytes(pk);
            }
            Msg::PairConfirm { ok } => {
                w.u8(4);
                w.u8(*ok as u8);
            }
            Msg::Ready => w.u8(10),
            Msg::Screens(v) => {
                w.u8(11);
                w.u16(v.len() as u16);
                for s in v {
                    w.str(&s.key);
                    w.str(&s.label);
                    w.i32(s.x);
                    w.i32(s.y);
                    w.i32(s.w);
                    w.i32(s.h);
                    w.f32(s.scale);
                }
            }
            Msg::Layout { version, places } => {
                w.u8(12);
                w.u64(*version);
                w.u16(places.len() as u16);
                for (k, x, y) in places {
                    w.str(k);
                    w.f32(*x);
                    w.f32(*y);
                }
            }
            Msg::Enter { x, y } => {
                w.u8(20);
                w.i32(*x);
                w.i32(*y);
            }
            Msg::Move { x, y } => {
                w.u8(21);
                w.i32(*x);
                w.i32(*y);
            }
            Msg::Leave => w.u8(22),
            Msg::Button { b, down } => {
                w.u8(23);
                w.u8(*b);
                w.u8(*down as u8);
            }
            Msg::Wheel { delta, horizontal } => {
                w.u8(24);
                w.i16(*delta);
                w.u8(*horizontal as u8);
            }
            Msg::Key { vk, scan, flags } => {
                w.u8(25);
                w.u16(*vk);
                w.u16(*scan);
                w.u8(*flags);
            }
            Msg::Take => w.u8(26),
            Msg::Ping { t } => {
                w.u8(30);
                w.u64(*t);
            }
            Msg::Pong { t } => {
                w.u8(31);
                w.u64(*t);
            }
            Msg::Bye => w.u8(40),
            Msg::Clip { text } => {
                w.u8(50);
                w.blob(text.as_bytes());
            }
            Msg::Identify => w.u8(51),
            Msg::MainScreen { key } => {
                w.u8(55);
                w.str(key);
            }
            Msg::DispAsk { key } => {
                w.u8(56);
                w.str(key);
            }
            Msg::DispInfo { key, scale, scale_rec, scales, res, res_rec, modes, orient, pending } => {
                w.u8(57);
                w.str(key);
                w.i32(*scale);
                w.i32(*scale_rec);
                w.u16(scales.len().min(32) as u16);
                for v in scales.iter().take(32) {
                    w.i32(*v);
                }
                for v in [res.0, res.1, res_rec.0, res_rec.1] {
                    w.i32(v);
                }
                w.u16(modes.len().min(200) as u16);
                for (a, b) in modes.iter().take(200) {
                    w.i32(*a);
                    w.i32(*b);
                }
                w.i32(*orient);
                w.i32(*pending);
            }
            Msg::DispSet { key, op, a, b, c } => {
                w.u8(58);
                w.str(key);
                w.u8(*op);
                w.i32(*a);
                w.i32(*b);
                w.i32(*c);
            }
            Msg::ShelfList(v) => {
                w.u8(52);
                w.u16(v.len().min(200) as u16);
                for (id, name, size, dir) in v.iter().take(200) {
                    w.u64(*id);
                    w.str(name);
                    w.u64(*size);
                    w.u8(*dir as u8);
                }
            }
            Msg::ShelfHashes(v) => {
                w.u8(54);
                w.u16(v.len().min(200) as u16);
                for (id, h) in v.iter().take(200) {
                    w.u64(*id);
                    w.u64(*h);
                }
            }
            Msg::ShelfGet(v) => {
                w.u8(53);
                w.u16(v.len().min(200) as u16);
                for id in v.iter().take(200) {
                    w.u64(*id);
                }
            }
            Msg::Batch { id, kind, count, total } => {
                w.u8(60);
                w.u64(*id);
                w.u8(*kind);
                w.0.extend_from_slice(&count.to_le_bytes());
                w.u64(*total);
            }
            Msg::FileHead { rel, size, dir } => {
                w.u8(61);
                w.blob(rel.as_bytes());
                w.u64(*size);
                w.u8(*dir as u8);
            }
            Msg::Data(b) => {
                w.u8(62);
                w.blob(b);
            }
            Msg::BatchEnd => w.u8(63),
            Msg::BatchAck { ok } => {
                w.u8(64);
                w.u8(*ok as u8);
            }
        }
        w.0
    }

    pub fn decode(b: &[u8]) -> Option<Msg> {
        let mut r = R(b, 0);
        Some(match r.u8()? {
            1 => Msg::Hello { proto: r.u16()?, id: r.u64()?, name: r.str()?, nonce: r.arr()?, purpose: r.u8().unwrap_or(FOR_SESSION) },
            2 => Msg::PairReq { proto: r.u16()?, id: r.u64()?, name: r.str()?, pk: r.arr()? },
            3 => Msg::PairResp { id: r.u64()?, name: r.str()?, pk: r.arr()? },
            4 => Msg::PairConfirm { ok: r.u8()? != 0 },
            10 => Msg::Ready,
            11 => {
                let n = r.u16()? as usize;
                let mut v = Vec::with_capacity(n.min(16));
                for _ in 0..n {
                    v.push(ScreenInfo { key: r.str()?, label: r.str()?, x: r.i32()?, y: r.i32()?, w: r.i32()?, h: r.i32()?, scale: r.f32()? });
                }
                Msg::Screens(v)
            }
            12 => {
                let version = r.u64()?;
                let n = r.u16()? as usize;
                let mut places = Vec::with_capacity(n.min(32));
                for _ in 0..n {
                    places.push((r.str()?, r.f32()?, r.f32()?));
                }
                Msg::Layout { version, places }
            }
            20 => Msg::Enter { x: r.i32()?, y: r.i32()? },
            21 => Msg::Move { x: r.i32()?, y: r.i32()? },
            22 => Msg::Leave,
            23 => Msg::Button { b: r.u8()?, down: r.u8()? != 0 },
            24 => Msg::Wheel { delta: r.i16()?, horizontal: r.u8()? != 0 },
            25 => Msg::Key { vk: r.u16()?, scan: r.u16()?, flags: r.u8()? },
            26 => Msg::Take,
            30 => Msg::Ping { t: r.u64()? },
            31 => Msg::Pong { t: r.u64()? },
            40 => Msg::Bye,
            50 => Msg::Clip { text: String::from_utf8(r.blob()?).ok()? },
            51 => Msg::Identify,
            55 => Msg::MainScreen { key: r.str()? },
            56 => Msg::DispAsk { key: r.str()? },
            57 => {
                let key = r.str()?;
                let (scale, scale_rec) = (r.i32()?, r.i32()?);
                let n = r.u16()? as usize;
                let mut scales = Vec::with_capacity(n.min(32));
                for _ in 0..n {
                    scales.push(r.i32()?);
                }
                let (rw, rh, qw, qh) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
                let n = r.u16()? as usize;
                let mut modes = Vec::with_capacity(n.min(200));
                for _ in 0..n {
                    modes.push((r.i32()?, r.i32()?));
                }
                Msg::DispInfo { key, scale, scale_rec, scales, res: (rw, rh), res_rec: (qw, qh), modes, orient: r.i32()?, pending: r.i32()? }
            }
            58 => Msg::DispSet { key: r.str()?, op: r.u8()?, a: r.i32()?, b: r.i32()?, c: r.i32()? },
            52 => {
                let n = r.u16()? as usize;
                let mut v = Vec::with_capacity(n.min(200));
                for _ in 0..n {
                    v.push((r.u64()?, r.str()?, r.u64()?, r.u8()? != 0));
                }
                Msg::ShelfList(v)
            }
            54 => {
                let n = r.u16()? as usize;
                let mut v = Vec::with_capacity(n.min(200));
                for _ in 0..n {
                    v.push((r.u64()?, r.u64()?));
                }
                Msg::ShelfHashes(v)
            }
            53 => {
                let n = r.u16()? as usize;
                let mut v = Vec::with_capacity(n.min(200));
                for _ in 0..n {
                    v.push(r.u64()?);
                }
                Msg::ShelfGet(v)
            }
            60 => Msg::Batch { id: r.u64()?, kind: r.u8()?, count: r.u32()?, total: r.u64()? },
            61 => Msg::FileHead { rel: String::from_utf8(r.blob()?).ok()?, size: r.u64()?, dir: r.u8()? != 0 },
            62 => Msg::Data(r.blob()?),
            63 => Msg::BatchEnd,
            64 => Msg::BatchAck { ok: r.u8()? != 0 },
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).expect("random");
    b
}

fn sha(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// One side of a pairing: a fresh X25519 key.
pub struct PairKey {
    secret: x25519_dalek::StaticSecret,
    pub public: [u8; 32],
}

impl PairKey {
    pub fn new() -> PairKey {
        let secret = x25519_dalek::StaticSecret::from(random::<32>());
        let public = x25519_dalek::PublicKey::from(&secret).to_bytes();
        PairKey { secret, public }
    }

    /// With the other side's public key: (the 6-digit code to compare, the
    /// pair's long-term secret). Both sides get the same two.
    pub fn finish(&self, my_id: u64, their_id: u64, their_pk: &[u8; 32]) -> (u32, [u8; 32]) {
        let shared = self.secret.diffie_hellman(&x25519_dalek::PublicKey::from(*their_pk)).to_bytes();
        let (lo, hi) = if my_id < their_id { (self.public, *their_pk) } else { (*their_pk, self.public) };
        let c = sha(&[b"marisland-link code", &shared, &lo, &hi]);
        let code = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) % 1_000_000;
        let secret = sha(&[b"marisland-link secret", &shared, &lo, &hi]);
        (code, secret)
    }
}

/// One direction of a session.
pub struct Cipher {
    aead: Aes256Gcm,
    counter: u64,
}

impl Cipher {
    fn new(key: [u8; 32]) -> Cipher {
        Cipher { aead: Aes256Gcm::new_from_slice(&key).expect("key"), counter: 0 }
    }
    fn nonce(&self) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[..8].copy_from_slice(&self.counter.to_le_bytes());
        n
    }
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let n = self.nonce();
        self.counter += 1;
        self.aead.encrypt(Nonce::from_slice(&n), plain).expect("seal")
    }
    pub fn open(&mut self, sealed: &[u8]) -> Option<Vec<u8>> {
        let n = self.nonce();
        let out = self.aead.decrypt(Nonce::from_slice(&n), sealed).ok()?;
        self.counter += 1;
        Some(out)
    }
}

/// (sending, receiving) ciphers for a session with the pair `secret`.
pub fn session(secret: &[u8; 32], my_id: u64, their_id: u64, my_nonce: &[u8; 16], their_nonce: &[u8; 16]) -> (Cipher, Cipher) {
    let (lo_id, hi_id) = (my_id.min(their_id), my_id.max(their_id));
    let (lo_n, hi_n) = if my_id < their_id { (my_nonce, their_nonce) } else { (their_nonce, my_nonce) };
    let t = [&lo_id.to_le_bytes()[..], &hi_id.to_le_bytes(), lo_n, hi_n].concat();
    let up = sha(&[b"marisland-link lo->hi", secret, &t]);
    let down = sha(&[b"marisland-link hi->lo", secret, &t]);
    if my_id < their_id {
        (Cipher::new(up), Cipher::new(down))
    } else {
        (Cipher::new(down), Cipher::new(up))
    }
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

pub const MAX_FRAME: usize = 1 << 20;

pub fn write_plain(w: &mut impl Write, m: &Msg) -> std::io::Result<()> {
    let body = m.encode();
    let mut f = Vec::with_capacity(body.len() + 5);
    f.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    f.push(0);
    f.extend_from_slice(&body);
    w.write_all(&f)
}

pub fn write_sealed(w: &mut impl Write, c: &mut Cipher, m: &Msg) -> std::io::Result<()> {
    let body = c.seal(&m.encode());
    let mut f = Vec::with_capacity(body.len() + 5);
    f.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    f.push(1);
    f.extend_from_slice(&body);
    w.write_all(&f)
}

/// The next frame: (sealed?, payload).
pub fn read_frame(r: &mut impl Read) -> std::io::Result<(bool, Vec<u8>)> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_FRAME {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "bad frame"));
    }
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    let sealed = b[0] == 1;
    b.remove(0);
    Ok((sealed, b))
}

pub fn bad(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string())
}

pub fn read_plain(r: &mut impl Read) -> std::io::Result<Msg> {
    let (sealed, b) = read_frame(r)?;
    if sealed {
        return Err(bad("expected a greeting"));
    }
    Msg::decode(&b).ok_or_else(|| bad("unknown message"))
}

pub fn read_sealed(r: &mut impl Read, c: &mut Cipher) -> std::io::Result<Msg> {
    let (sealed, b) = read_frame(r)?;
    if !sealed {
        return Err(bad("unsealed message in a session"));
    }
    let plain = c.open(&b).ok_or_else(|| bad("couldn't unseal (wrong pairing?)"))?;
    Msg::decode(&plain).ok_or_else(|| bad("unknown message"))
}

/// Start a session. When we're answering, `first` is their greeting
/// (already read). `secret_for` finds the pair secret for a PC id. Returns
/// (their id, their name, send cipher, receive cipher).
pub fn handshake<S: Read + Write>(s: &mut S, my_id: u64, my_name: &str, first: Option<Msg>, secret_for: impl Fn(u64) -> Option<[u8; 32]>) -> std::io::Result<(u64, String, Cipher, Cipher)> {
    handshake_for(s, my_id, my_name, first, secret_for, FOR_SESSION).map(|(id, name, tx, rx, _)| (id, name, tx, rx))
}

/// `handshake`, saying what the connection is for; returns theirs too.
pub fn handshake_for<S: Read + Write>(s: &mut S, my_id: u64, my_name: &str, first: Option<Msg>, secret_for: impl Fn(u64) -> Option<[u8; 32]>, purpose: u8) -> std::io::Result<(u64, String, Cipher, Cipher, u8)> {
    let my_nonce = random::<16>();
    write_plain(s, &Msg::Hello { proto: PROTO, id: my_id, name: my_name.to_string(), nonce: my_nonce, purpose })?;
    let theirs = match first {
        Some(m) => m,
        None => read_plain(s)?,
    };
    let Msg::Hello { proto, id, name, nonce, purpose: theirs_for } = theirs else { return Err(bad("expected hello")) };
    if proto != PROTO {
        return Err(bad("different Phemio version on the other PC"));
    }
    let secret = secret_for(id).ok_or_else(|| bad("not paired"))?;
    let (mut tx, mut rx) = session(&secret, my_id, id, &my_nonce, &nonce);
    write_sealed(s, &mut tx, &Msg::Ready)?;
    match read_sealed(s, &mut rx)? {
        Msg::Ready => Ok((id, name, tx, rx, theirs_for.max(purpose))),
        _ => Err(bad("expected ready")),
    }
}

/// Pairing, the side that asked: (their id, their name, code, secret).
pub fn pair_ask<S: Read + Write>(s: &mut S, my_id: u64, my_name: &str, key: &PairKey) -> std::io::Result<(u64, String, u32, [u8; 32])> {
    write_plain(s, &Msg::PairReq { proto: PROTO, id: my_id, name: my_name.to_string(), pk: key.public })?;
    match read_plain(s)? {
        Msg::PairResp { id, name, pk } => {
            let (code, secret) = key.finish(my_id, id, &pk);
            Ok((id, name, code, secret))
        }
        _ => Err(bad("expected a pairing answer")),
    }
}

/// Pairing, the side that was asked (their request already read).
pub fn pair_answer<S: Read + Write>(s: &mut S, my_id: u64, my_name: &str, key: &PairKey, their_id: u64, their_pk: &[u8; 32]) -> std::io::Result<(u32, [u8; 32])> {
    write_plain(s, &Msg::PairResp { id: my_id, name: my_name.to_string(), pk: key.public })?;
    Ok(key.finish(my_id, their_id, their_pk))
}

/// Both sides say yes (or no) once you've compared the codes.
pub fn pair_confirm<S: Read + Write>(s: &mut S, mine: bool) -> std::io::Result<bool> {
    write_plain(s, &Msg::PairConfirm { ok: mine })?;
    match read_plain(s)? {
        Msg::PairConfirm { ok } => Ok(ok && mine),
        _ => Err(bad("expected a confirmation")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn messages_round_trip() {
        let all = vec![
            Msg::Hello { proto: 2, id: 7, name: "LAPTOP-1".into(), nonce: [9; 16], purpose: 1 },
            Msg::Clip { text: "xin chào ".repeat(20000) },
            Msg::ShelfList(vec![(5, "report.pdf".into(), 12345, false), (6, "Photos".into(), 0, true)]),
            Msg::ShelfHashes(vec![(5, 0xdead_beef), (6, 7)]),
            Msg::ShelfGet(vec![5, 6]),
            Msg::Batch { id: 9, kind: BATCH_CLIPBOARD, count: 3, total: 1 << 33 },
            Msg::FileHead { rel: "Photos\\a.jpg".into(), size: 999, dir: false },
            Msg::Data(vec![7; 256 * 1024]),
            Msg::BatchEnd,
            Msg::BatchAck { ok: true },
            Msg::Identify,
            Msg::MainScreen { key: "\\\\.\\DISPLAY2".into() },
            Msg::DispAsk { key: "D2".into() },
            Msg::DispInfo { key: "D2".into(), scale: 150, scale_rec: 150, scales: vec![100, 125, 150], res: (2880, 1800), res_rec: (2880, 1800), modes: vec![(2880, 1800), (1920, 1200)], orient: 1, pending: 9 },
            Msg::DispSet { key: "D2".into(), op: 2, a: 1920, b: 1080, c: 0 },
            Msg::Screens(vec![ScreenInfo { key: "\\\\.\\DISPLAY1".into(), label: "Dell".into(), x: -1920, y: 0, w: 1920, h: 1080, scale: 1.25 }]),
            Msg::Layout { version: 3, places: vec![("a/b".into(), 1.5, -2.0)] },
            Msg::Move { x: -5, y: 1200 },
            Msg::Key { vk: 0x41, scan: 0x1e, flags: 2 },
            Msg::Wheel { delta: -120, horizontal: false },
            Msg::Button { b: 1, down: true },
            Msg::Ping { t: 123456789 },
        ];
        for m in all {
            assert_eq!(Msg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn pairing_gives_both_sides_the_same_code_and_secret() {
        let (a, b) = (PairKey::new(), PairKey::new());
        let (ca, sa) = a.finish(1, 2, &b.public);
        let (cb, sb) = b.finish(2, 1, &a.public);
        assert_eq!(ca, cb);
        assert_eq!(sa, sb);
        assert!(ca < 1_000_000);
    }

    #[test]
    fn a_real_connection() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        // PC 2 answers.
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let key = PairKey::new();
            let Msg::PairReq { id, pk, .. } = read_plain(&mut s).unwrap() else { panic!() };
            let (code, secret) = pair_answer(&mut s, 2, "LAPTOP-2", &key, id, &pk).unwrap();
            assert!(pair_confirm(&mut s, true).unwrap());
            // The session on the same connection.
            let first = read_plain(&mut s).unwrap();
            let (pid, _, mut tx, mut rx) = handshake(&mut s, 2, "LAPTOP-2", Some(first), |_| Some(secret)).unwrap();
            assert_eq!(pid, 1);
            let m = read_sealed(&mut s, &mut rx).unwrap();
            assert_eq!(m, Msg::Move { x: 10, y: 20 });
            write_sealed(&mut s, &mut tx, &Msg::Pong { t: 5 }).unwrap();
            (code, secret)
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_nodelay(true).unwrap();
        let key = PairKey::new();
        let (id, name, code, secret) = pair_ask(&mut s, 1, "LAPTOP-1", &key).unwrap();
        assert_eq!((id, name.as_str()), (2, "LAPTOP-2"));
        assert!(pair_confirm(&mut s, true).unwrap());
        let (_, _, mut tx, mut rx) = handshake(&mut s, 1, "LAPTOP-1", None, |_| Some(secret)).unwrap();
        let t0 = std::time::Instant::now();
        write_sealed(&mut s, &mut tx, &Msg::Move { x: 10, y: 20 }).unwrap();
        assert_eq!(read_sealed(&mut s, &mut rx).unwrap(), Msg::Pong { t: 5 });
        let rtt = t0.elapsed();
        let (code2, secret2) = t.join().unwrap();
        assert_eq!((code, secret), (code2, secret2));
        eprintln!("sealed round trip over loopback: {rtt:?}");
    }

    #[test]
    fn a_stranger_cannot_join() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let first = read_plain(&mut s).unwrap();
            handshake(&mut s, 2, "B", Some(first), |_| Some([1; 32])).is_err()
        });
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        // Knows the protocol, not the secret.
        let r = handshake(&mut s, 9, "intruder", None, |_| Some([2; 32]));
        assert!(r.is_err());
        assert!(t.join().unwrap());
    }

    #[test]
    fn tampering_is_caught() {
        let (mut tx, _) = session(&[3; 32], 1, 2, &[1; 16], &[2; 16]);
        let (_, mut rx) = session(&[3; 32], 2, 1, &[2; 16], &[1; 16]);
        let mut sealed = tx.seal(&Msg::Key { vk: 13, scan: 28, flags: 0 }.encode());
        sealed[1] ^= 1;
        assert!(rx.open(&sealed).is_none());
    }

    #[test]
    fn sealing_is_fast() {
        let (mut tx, _) = session(&[3; 32], 1, 2, &[1; 16], &[2; 16]);
        let m = Msg::Move { x: 1000, y: 500 }.encode();
        let t0 = std::time::Instant::now();
        for _ in 0..100_000 {
            std::hint::black_box(tx.seal(&m));
        }
        let per = t0.elapsed() / 100_000;
        eprintln!("seal one mouse move: {per:?}");
        assert!(per < std::time::Duration::from_micros(50));
    }
}

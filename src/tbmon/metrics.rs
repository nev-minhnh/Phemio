//! System metrics for the taskbar monitor — a Rust port of TaskbarMonitor's
//! Metrics.cs / Adapters.cs. Same sources Task Manager uses:
//! PDH counters for CPU / speed / disk I/O / GPU / NPU, GlobalMemoryStatusEx
//! for RAM, GetIfTable2 for network, GetDiskFreeSpaceEx for drives.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Performance::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::GetSystemTimes;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f64,
    pub mhz: Option<f64>,
    pub ram: f64,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub gpu: Option<f64>,
    pub npu: Option<f64>,
    pub disk_read: f64,
    pub disk_write: f64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub rx: f64,
    pub tx: f64,
}

const NOCAP100: PDH_FMT = PDH_FMT(0x0000_8000);

fn fmt() -> PDH_FMT {
    PDH_FMT(PDH_FMT_DOUBLE.0 | NOCAP100.0)
}

/// A PDH query holding several single-value counters.
struct Pdh {
    q: PDH_HQUERY,
}

impl Pdh {
    fn new() -> Pdh {
        let mut q = PDH_HQUERY::default();
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut q) != 0 {
                q = PDH_HQUERY::default();
            }
        }
        Pdh { q }
    }
    fn add(&self, path: &str) -> Option<PDH_HCOUNTER> {
        if self.q.0.is_null() {
            return None;
        }
        let mut c = PDH_HCOUNTER::default();
        let p = HSTRING::from(path);
        unsafe { (PdhAddEnglishCounterW(self.q, &p, 0, &mut c) == 0).then_some(c) }
    }
    fn collect(&self) {
        if !self.q.0.is_null() {
            unsafe {
                PdhCollectQueryData(self.q);
            }
        }
    }
    fn get(&self, c: Option<PDH_HCOUNTER>) -> Option<f64> {
        let c = c?;
        let mut v = PDH_FMT_COUNTERVALUE::default();
        unsafe {
            if PdhGetFormattedCounterValue(c, fmt(), None, &mut v) != 0 || v.CStatus > 1 {
                return None;
            }
            Some(v.Anonymous.doubleValue)
        }
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        if !self.q.0.is_null() {
            unsafe {
                PdhCloseQuery(self.q);
            }
        }
    }
}

/// Wildcard "\GPU Engine(*)\Utilization Percentage": per process per engine.
/// Like Task Manager we sum per engine and report the busiest engine. An
/// optional LUID filter picks the adapter (GPU vs NPU).
struct EngineCounter {
    pdh: Pdh,
    counter: Option<PDH_HCOUNTER>,
    filter: Option<Box<dyn Fn(&str) -> bool + Send>>,
    matched_any: bool,
}

impl EngineCounter {
    fn new(path: &str, filter: Option<Box<dyn Fn(&str) -> bool + Send>>) -> Option<EngineCounter> {
        let pdh = Pdh::new();
        let counter = pdh.add(path)?;
        pdh.collect();
        Some(EngineCounter { pdh, counter: Some(counter), filter, matched_any: false })
    }

    fn busiest(&mut self) -> f64 {
        let Some(c) = self.counter else { return 0.0 };
        self.pdh.collect();
        unsafe {
            let mut size = 0u32;
            let mut count = 0u32;
            let st = PdhGetFormattedCounterArrayW(c, fmt(), &mut size, &mut count, None);
            if st != PDH_MORE_DATA || size == 0 {
                return 0.0;
            }
            // u64 buffer keeps the items 8-byte aligned.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(c, fmt(), &mut size, &mut count, Some(items)) != 0 {
                return 0.0;
            }
            let mut per_engine: HashMap<String, f64> = HashMap::new();
            for i in 0..count as usize {
                let it = &*items.add(i);
                let name = it.szName.to_string().unwrap_or_default().to_lowercase();
                // "pid_1234_luid_0x00000000_0x0000d1b5_phys_0_eng_3_engtype_3d"
                let key = match name.find("luid_") {
                    Some(at) => name[at..].to_string(),
                    None => name.clone(),
                };
                let luid = match key.find("_phys") {
                    Some(p) => key[..p].to_string(),
                    None => key.clone(),
                };
                if let Some(f) = &self.filter {
                    if !f(&luid) {
                        continue;
                    }
                }
                self.matched_any = true;
                if it.FmtValue.CStatus > 1 {
                    continue;
                }
                *per_engine.entry(key).or_insert(0.0) += it.FmtValue.Anonymous.doubleValue;
            }
            per_engine.values().cloned().fold(0.0, f64::max).clamp(0.0, 100.0)
        }
    }
}

// ---------------------------------------------------------------------------
// Graphics / compute adapters via D3DKMT (gdi32), loaded dynamically
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AdapterInfo {
    h_adapter: u32,
    luid_low: u32,
    luid_high: i32,
    num_sources: u32,
    precise: i32,
}

#[repr(C)]
struct EnumAdapters2 {
    num: u32,
    adapters: *mut AdapterInfo,
}

#[repr(C)]
struct EnumAdapters3 {
    filter: u64,
    num: u32,
    adapters: *mut AdapterInfo,
}

#[repr(C)]
struct QueryAdapterInfo {
    h_adapter: u32,
    kind: i32,
    data: *mut c_void,
    size: u32,
}

const KMTQAITYPE_ADAPTERREGISTRYINFO: i32 = 8;
const KMTQAITYPE_ADAPTERTYPE: i32 = 15;
const TYPE_RENDER: u32 = 1;
const TYPE_SOFTWARE: u32 = 1 << 2;
const TYPE_COMPUTE_ONLY: u32 = 1 << 11;

struct Adapter {
    luid: String,
    name: String,
    flags: u32,
}

impl Adapter {
    fn software(&self) -> bool {
        self.flags & TYPE_SOFTWARE != 0
    }
    fn npu(&self) -> bool {
        let n = self.name.to_lowercase();
        !self.software()
            && (self.flags & TYPE_COMPUTE_ONLY != 0
                || n.contains(" npu")
                || n.starts_with("npu")
                || n.contains("neural")
                || n.contains("ai boost")
                || n.contains("hexagon")
                || n.contains("compute accelerator"))
    }
    fn gpu(&self) -> bool {
        !self.software() && !self.npu() && self.flags & TYPE_RENDER != 0
    }
}

fn adapters() -> Vec<Adapter> {
    unsafe {
        let Ok(gdi) = LoadLibraryW(w!("gdi32.dll")) else { return vec![] };
        type Enum2 = unsafe extern "system" fn(*mut EnumAdapters2) -> i32;
        type Enum3 = unsafe extern "system" fn(*mut EnumAdapters3) -> i32;
        type Query = unsafe extern "system" fn(*mut QueryAdapterInfo) -> i32;
        type Close = unsafe extern "system" fn(*mut u32) -> i32;
        let e3: Option<Enum3> = GetProcAddress(gdi, s!("D3DKMTEnumAdapters3")).map(|f| std::mem::transmute(f));
        let e2: Option<Enum2> = GetProcAddress(gdi, s!("D3DKMTEnumAdapters2")).map(|f| std::mem::transmute(f));
        let q: Option<Query> = GetProcAddress(gdi, s!("D3DKMTQueryAdapterInfo")).map(|f| std::mem::transmute(f));
        let cl: Option<Close> = GetProcAddress(gdi, s!("D3DKMTCloseAdapter")).map(|f| std::mem::transmute(f));
        let (Some(q), Some(cl)) = (q, cl) else { return vec![] };

        let mut list: Vec<AdapterInfo> = Vec::new();
        if let Some(e3) = e3 {
            // IncludeComputeOnly: the only way to see NPUs.
            let mut a = EnumAdapters3 { filter: 1, num: 0, adapters: std::ptr::null_mut() };
            if e3(&mut a) == 0 && a.num > 0 {
                list = vec![AdapterInfo::default(); a.num as usize + 4];
                a.num = list.len() as u32;
                a.adapters = list.as_mut_ptr();
                if e3(&mut a) == 0 {
                    list.truncate(a.num as usize);
                } else {
                    list.clear();
                }
            }
        }
        if list.is_empty() {
            if let Some(e2) = e2 {
                let mut a = EnumAdapters2 { num: 0, adapters: std::ptr::null_mut() };
                if e2(&mut a) == 0 && a.num > 0 {
                    list = vec![AdapterInfo::default(); a.num as usize + 4];
                    a.num = list.len() as u32;
                    a.adapters = list.as_mut_ptr();
                    if e2(&mut a) == 0 {
                        list.truncate(a.num as usize);
                    } else {
                        list.clear();
                    }
                }
            }
        }
        let mut out = Vec::new();
        for ai in list {
            let mut flags = 0u32;
            let mut qi = QueryAdapterInfo { h_adapter: ai.h_adapter, kind: KMTQAITYPE_ADAPTERTYPE, data: &mut flags as *mut u32 as *mut c_void, size: 4 };
            if q(&mut qi) != 0 {
                flags = 0;
            }
            // D3DKMT_ADAPTERREGISTRYINFO: 4 × WCHAR[260]
            let mut reg = vec![0u16; 260 * 4];
            let mut qn = QueryAdapterInfo { h_adapter: ai.h_adapter, kind: KMTQAITYPE_ADAPTERREGISTRYINFO, data: reg.as_mut_ptr() as *mut c_void, size: (reg.len() * 2) as u32 };
            let name = if q(&mut qn) == 0 { crate::util::from_wide(&reg[..260]).trim().to_string() } else { String::new() };
            out.push(Adapter { luid: format!("luid_0x{:08x}_0x{:08x}", ai.luid_high as u32, ai.luid_low), name, flags });
            let mut h = ai.h_adapter;
            cl(&mut h);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Sampler
// ---------------------------------------------------------------------------

pub struct Sampler {
    pdh: Pdh,
    cpu_util: Option<PDH_HCOUNTER>,
    cpu_perf: Option<PDH_HCOUNTER>,
    cpu_freq: Option<PDH_HCOUNTER>,
    disk_r: Option<PDH_HCOUNTER>,
    disk_w: Option<PDH_HCOUNTER>,
    gpu: Option<EngineCounter>,
    npu: Option<EngineCounter>,
    npu_known: bool,
    prev_idle: u64,
    prev_total: u64,
    prev_net: Option<(u64, u64, Instant)>,
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl Sampler {
    pub fn new(want_gpu: bool, want_npu: bool) -> Sampler {
        let pdh = Pdh::new();
        let cpu_util = pdh.add(r"\Processor Information(_Total)\% Processor Utility");
        let cpu_perf = pdh.add(r"\Processor Information(_Total)\% Processor Performance");
        let cpu_freq = pdh.add(r"\Processor Information(_Total)\Processor Frequency");
        let disk_r = pdh.add(r"\PhysicalDisk(_Total)\Disk Read Bytes/sec");
        let disk_w = pdh.add(r"\PhysicalDisk(_Total)\Disk Write Bytes/sec");
        pdh.collect();

        let (mut gpu, mut npu, mut npu_known) = (None, None, false);
        if want_gpu || want_npu {
            let ad = adapters();
            let set = |f: &dyn Fn(&Adapter) -> bool| -> HashSet<String> { ad.iter().filter(|a| f(a)).map(|a| a.luid.clone()).collect() };
            let gpus = set(&|a| a.gpu());
            let npus = set(&|a| a.npu());
            let soft = set(&|a| a.software());
            const GPU_PATH: &str = r"\GPU Engine(*)\Utilization Percentage";
            if want_gpu {
                let (g2, n2, s2) = (gpus.clone(), npus.clone(), soft.clone());
                gpu = EngineCounter::new(
                    GPU_PATH,
                    Some(Box::new(move |l: &str| if !g2.is_empty() { g2.contains(l) } else { !n2.contains(l) && !s2.contains(l) })),
                );
            }
            if want_npu {
                if let Some(c) = EngineCounter::new(r"\NPU Engine(*)\Utilization Percentage", None) {
                    npu = Some(c);
                    npu_known = true;
                } else if !npus.is_empty() {
                    let n2 = npus.clone();
                    npu = EngineCounter::new(GPU_PATH, Some(Box::new(move |l: &str| n2.contains(l))));
                    npu_known = true;
                } else if !gpus.is_empty() {
                    let (g2, s2) = (gpus.clone(), soft.clone());
                    npu = EngineCounter::new(GPU_PATH, Some(Box::new(move |l: &str| !g2.contains(l) && !s2.contains(l))));
                }
            }
        }
        Sampler { pdh, cpu_util, cpu_perf, cpu_freq, disk_r, disk_w, gpu, npu, npu_known, prev_idle: 0, prev_total: 0, prev_net: None }
    }

    /// `drives`: letters like ['C', 'D']; empty = all fixed drives.
    pub fn sample(&mut self, drives: &[char], want_gpu: bool, want_npu: bool, want_space: bool) -> Snapshot {
        self.pdh.collect();
        let mut s = Snapshot::default();

        // CPU (Task Manager's "% Processor Utility"; GetSystemTimes fallback)
        let mut fallback = 0.0;
        unsafe {
            let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).is_ok() {
                let (idle, total) = (ft(i), ft(k) + ft(u));
                let (di, dt) = (idle.wrapping_sub(self.prev_idle), total.wrapping_sub(self.prev_total));
                if self.prev_total != 0 && dt > 0 {
                    fallback = (100.0 * (dt.saturating_sub(di)) as f64 / dt as f64).clamp(0.0, 100.0);
                }
                self.prev_idle = idle;
                self.prev_total = total;
            }
        }
        s.cpu = self.pdh.get(self.cpu_util).map(|v| v.clamp(0.0, 100.0)).unwrap_or(fallback);
        if let (Some(base), Some(perf)) = (self.pdh.get(self.cpu_freq), self.pdh.get(self.cpu_perf)) {
            if base > 0.0 {
                s.mhz = Some(base * perf / 100.0);
            }
        }

        // RAM
        unsafe {
            let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut m).is_ok() && m.ullTotalPhys > 0 {
                let used = m.ullTotalPhys - m.ullAvailPhys;
                s.ram = 100.0 * used as f64 / m.ullTotalPhys as f64;
                s.ram_used_gb = used as f64 / 1073741824.0;
                s.ram_total_gb = m.ullTotalPhys as f64 / 1073741824.0;
            }
        }

        // GPU / NPU
        if want_gpu {
            s.gpu = self.gpu.as_mut().map(|g| g.busiest());
        }
        if want_npu {
            if let Some(n) = self.npu.as_mut() {
                let v = n.busiest();
                if self.npu_known || n.matched_any {
                    s.npu = Some(v);
                }
            }
        }

        // Disk I/O
        s.disk_read = self.pdh.get(self.disk_r).unwrap_or(0.0).max(0.0);
        s.disk_write = self.pdh.get(self.disk_w).unwrap_or(0.0).max(0.0);

        // Disk space
        if want_space {
            unsafe {
                let mask = GetLogicalDrives();
                for i in 0..26u32 {
                    if mask & (1 << i) == 0 {
                        continue;
                    }
                    let letter = (b'A' + i as u8) as char;
                    if !drives.is_empty() && !drives.contains(&letter) {
                        continue;
                    }
                    let root = HSTRING::from(format!("{letter}:\\"));
                    if GetDriveTypeW(&root) != 3 {
                        continue; // DRIVE_FIXED
                    }
                    let (mut free, mut total) = (0u64, 0u64);
                    if GetDiskFreeSpaceExW(&root, None, Some(&mut total), Some(&mut free)).is_ok() {
                        s.disk_total += total;
                        s.disk_used += total.saturating_sub(free);
                    }
                }
            }
        }

        // Network (all active, non-loopback/tunnel, non-filter interfaces)
        let (mut rx, mut tx) = (0u64, 0u64);
        unsafe {
            let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut t) == NO_ERROR && !t.is_null() {
                let n = (*t).NumEntries as usize;
                let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), n);
                for r in rows {
                    let filter = r.InterfaceAndOperStatusFlags._bitfield & 0b10 != 0;
                    if r.OperStatus.0 != 1 || r.Type == 24 || r.Type == 131 || filter {
                        continue;
                    }
                    rx += r.InOctets;
                    tx += r.OutOctets;
                }
                FreeMibTable(t as *const c_void);
            }
        }
        let now = Instant::now();
        if let Some((prx, ptx, pt)) = self.prev_net {
            let dt = (now - pt).as_secs_f64();
            if dt > 0.0 {
                s.rx = rx.saturating_sub(prx) as f64 / dt;
                s.tx = tx.saturating_sub(ptx) as f64 / dt;
            }
        }
        self.prev_net = Some((rx, tx, now));
        s
    }
}

// ---------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------

fn num(v: f64) -> String {
    if v < 10.0 {
        format!("{v:.2}")
    } else if v < 100.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.0}")
    }
}

/// 0.42KB/s, 12.5KB/s, 300MB/s
pub fn rate(bps: f64) -> String {
    let mut v = bps / 1024.0;
    let mut unit = "KB/s";
    if v >= 1000.0 {
        v /= 1024.0;
        unit = "MB/s";
    }
    if v >= 1000.0 {
        v /= 1024.0;
        unit = "GB/s";
    }
    format!("{}{unit}", num(v))
}

/// Narrow form for vertical taskbars: 5.1K, 419K, 2.3M
pub fn rate_short(bps: f64) -> String {
    let mut v = bps / 1024.0;
    let mut unit = "K";
    if v >= 1000.0 {
        v /= 1024.0;
        unit = "M";
    }
    if v >= 1000.0 {
        v /= 1024.0;
        unit = "G";
    }
    let n = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
    format!("{n}{unit}")
}

/// 25GB, 512GB, 1.8TB
pub fn size(bytes: u64) -> String {
    let gb = bytes as f64 / 1073741824.0;
    if gb < 1000.0 {
        format!("{gb:.0}GB")
    } else {
        format!("{:.1}TB", gb / 1024.0)
    }
}

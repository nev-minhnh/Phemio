//! File shelf: drop files on the island, drag them out again (copied/linked,
//! never moved). Only paths are kept; the files stay where they are.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;

use crate::config;
use crate::gfx::Image;
use crate::util;

pub struct Item {
    pub path: String,
    pub name: String,
    pub icon: Option<Image>,
    icon_px: u32,
}

pub struct Shelf {
    pub items: Vec<Item>,
}

impl Shelf {
    pub fn load() -> Shelf {
        let mut items = Vec::new();
        if let Ok(t) = std::fs::read_to_string(config::data_file("shelf.txt")) {
            for l in t.lines() {
                if !l.trim().is_empty() && std::path::Path::new(l).exists() {
                    items.push(item(l));
                }
            }
        }
        crate::link::clip::shelf_changed(items.iter().map(|i| i.path.clone()).collect());
        Shelf { items }
    }

    fn save(&self) {
        let s: String = self.items.iter().map(|i| format!("{}\n", i.path)).collect();
        let _ = std::fs::write(config::data_file("shelf.txt"), s);
        // The other PC sees the shelf too.
        crate::link::clip::shelf_changed(self.items.iter().map(|i| i.path.clone()).collect());
    }

    pub fn add(&mut self, paths: Vec<String>) {
        for p in paths.into_iter().rev() {
            self.items.retain(|i| !i.path.eq_ignore_ascii_case(&p));
            self.items.insert(0, item(&p));
        }
        self.items.truncate(60);
        self.save();
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.items.len() {
            self.items.remove(i);
            self.save();
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.save();
    }

    /// Load (crisp, exact-size) icons / thumbnails for visible items.
    pub fn ensure_icons(&mut self, px: u32, range: std::ops::Range<usize>) {
        for i in range {
            let Some(it) = self.items.get_mut(i) else { break };
            if it.icon_px != px {
                it.icon = shell_image(&it.path, px);
                it.icon_px = px;
            }
        }
    }
}

fn item(p: &str) -> Item {
    let name = std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    Item { path: p.to_string(), name, icon: None, icon_px: 0 }
}

/// Thumbnail for pictures/videos, the file-type icon otherwise.
pub fn shell_image(path: &str, px: u32) -> Option<Image> {
    unsafe {
        let wp = util::wide(path);
        let f: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wp.as_ptr()), None).ok()?;
        let hb = f.GetImage(SIZE { cx: px as i32, cy: px as i32 }, SIIGBF_RESIZETOFIT).ok()?;
        let img = Image::from_hbitmap(hb, px);
        let _ = DeleteObject(hb.into());
        img.ok().filter(|i| !i.is_blank())
    }
}

// ---------------------------------------------------------------------------
// Drop target (files dragged onto the island)
// ---------------------------------------------------------------------------

static ACCEPT: AtomicBool = AtomicBool::new(false);

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn files_of(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let fmt = hdrop_format();
        if let Ok(mut med) = data.GetData(&fmt) {
            let hdrop = HDROP(med.u.hGlobal.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            for i in 0..n.min(200) {
                let len = DragQueryFileW(hdrop, i, None);
                let mut buf = vec![0u16; len as usize + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                out.push(util::from_wide(&buf));
            }
            ReleaseStgMedium(&mut med);
        }
    }
    out
}

#[implement(IDropTarget)]
pub struct DropTarget;

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let ok = data.ok().map(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK).unwrap_or(false);
        ACCEPT.store(ok, Ordering::Relaxed);
        unsafe {
            *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        if ok {
            if let Some(i) = crate::app().island.as_mut() {
                i.drag_hover(true);
            }
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            *effect = if ACCEPT.load(Ordering::Relaxed) { DROPEFFECT_COPY } else { DROPEFFECT_NONE };
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if let Some(i) = crate::app().island.as_mut() {
            i.drag_hover(false);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let files = data.ok().map(files_of).unwrap_or_default();
        unsafe {
            *effect = if files.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY };
        }
        if let Some(i) = crate::app().island.as_mut() {
            i.drop_files(files);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Drag source (files dragged out of the island)
// ---------------------------------------------------------------------------

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, esc: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if esc.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }
    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Start a shell drag of the given files (blocks until dropped). Returns true if dropped.
pub fn drag_out(paths: &[String]) -> bool {
    unsafe {
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for p in paths {
            let w = util::wide(p);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                pidls.push(pidl);
            }
        }
        let mut dropped = false;
        if !pidls.is_empty() {
            let refs: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
            if let Ok(arr) = SHCreateShellItemArrayFromIDLists(&refs) {
                if let Ok(data) = arr.BindToHandler::<_, IDataObject>(None, &BHID_DataObject) {
                    let src: IDropSource = DropSource.into();
                    let mut eff = DROPEFFECT_NONE;
                    let hr = DoDragDrop(&data, &src, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut eff);
                    dropped = hr == DRAGDROP_S_DROP && eff != DROPEFFECT_NONE;
                }
            }
        }
        for p in pidls {
            CoTaskMemFree(Some(p as *const _));
        }
        dropped
    }
}

pub fn register(hwnd: HWND) {
    unsafe {
        let t: IDropTarget = DropTarget.into();
        let _ = RegisterDragDrop(hwnd, &t);
    }
}

pub fn revoke(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

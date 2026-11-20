//! File operations used by the library UI: Recycle Bin deletion, "show in
//! Explorer", copying files to the clipboard (paste into Discord, a browser
//! upload field, …).

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::{
    DROPFILES, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
    SHFILEOPSTRUCTW, SHFileOperationW,
};
use windows::core::PCWSTR;

/// `<clip>.markers.json` written by the recorder.
pub fn sidecar(clip: &Path) -> PathBuf {
    let mut s = clip.as_os_str().to_owned();
    s.push(".markers.json");
    PathBuf::from(s)
}

/// Delete a file: to the Recycle Bin (default) or permanently.
pub fn delete_file(path: &Path, permanent: bool) -> std::io::Result<()> {
    if permanent {
        return std::fs::remove_file(path);
    }
    let from: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0, 0]) // double-NUL terminated list
        .collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` outlives the call and is double-NUL terminated.
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 || op.fAnyOperationsAborted.as_bool() {
        return Err(std::io::Error::other(format!("recycle failed (code {rc})")));
    }
    Ok(())
}

/// Open Explorer with the file selected.
pub fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
}

/// Open a file with its default application.
pub fn open_default(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// Put files on the clipboard as CF_HDROP ("copy file"), like Explorer does.
pub fn copy_files_to_clipboard(paths: &[PathBuf]) -> windows::core::Result<()> {
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.as_os_str().to_string_lossy().encode_utf16());
        list.push(0);
    }
    list.push(0);
    let header = size_of::<DROPFILES>();
    let bytes = header + list.len() * 2;
    // SAFETY: global memory sized for header + list; ownership passes to the
    // clipboard on success, freed by us on failure.
    unsafe {
        let mem: HGLOBAL = GlobalAlloc(GHND, bytes)?;
        let ptr = GlobalLock(mem) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(windows::core::Error::from_thread());
        }
        let df = DROPFILES {
            pFiles: header as u32,
            fWide: true.into(),
            ..Default::default()
        };
        std::ptr::copy_nonoverlapping((&df as *const DROPFILES).cast::<u8>(), ptr, header);
        std::ptr::copy_nonoverlapping(list.as_ptr().cast::<u8>(), ptr.add(header), list.len() * 2);
        let _ = GlobalUnlock(mem);
        OpenClipboard(None)?;
        let _ = EmptyClipboard();
        let res = SetClipboardData(u32::from(CF_HDROP.0), Some(HANDLE(mem.0)));
        let _ = CloseClipboard();
        if res.is_err() {
            let _ = GlobalFree(Some(mem));
        }
        res.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_name() {
        assert_eq!(
            sidecar(Path::new(r"C:\a\b.mp4")),
            PathBuf::from(r"C:\a\b.mp4.markers.json")
        );
    }

    #[test]
    fn permanent_delete() {
        let p = std::env::temp_dir().join(format!("hc-del-{}.tmp", std::process::id()));
        std::fs::write(&p, b"x").unwrap();
        delete_file(&p, true).unwrap();
        assert!(!p.exists());
    }
}

//! Repository file watcher using ReadDirectoryChangesW on a background thread.
//! The thread blocks inside the kernel, so an idle repository costs zero CPU.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Arc;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub struct Watcher {
    handle: Arc<AtomicIsize>,
    stop: Arc<AtomicBool>,
}

impl Watcher {
    pub fn start(root: &Path, hwnd: HWND, msg: u32) -> Option<Watcher> {
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let handle = Arc::new(AtomicIsize::new(h as isize));
        let stop = Arc::new(AtomicBool::new(false));
        let (hc, sc) = (handle.clone(), stop.clone());
        let hwnd_val = hwnd as isize;
        std::thread::Builder::new()
            .name("watcher".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut buf = vec![0u32; 16 * 1024]; // 64 KiB, DWORD aligned
                loop {
                    let mut returned = 0u32;
                    let h = hc.load(Ordering::SeqCst) as HANDLE;
                    let ok = unsafe {
                        ReadDirectoryChangesW(
                            h,
                            buf.as_mut_ptr() as *mut _,
                            (buf.len() * 4) as u32,
                            1,
                            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
                            &mut returned,
                            std::ptr::null_mut(),
                            None,
                        )
                    };
                    if sc.load(Ordering::SeqCst) {
                        break;
                    }
                    if ok == 0 {
                        break;
                    }
                    let relevant = if returned == 0 { true } else { unsafe { any_relevant(buf.as_ptr() as *const u8) } };
                    if relevant {
                        unsafe {
                            PostMessageW(hwnd_val as HWND, msg, 0, 0);
                        }
                    }
                }
                unsafe {
                    CloseHandle(hc.load(Ordering::SeqCst) as HANDLE);
                }
            })
            .ok()?;
        Some(Watcher { handle, stop })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe {
            CancelIoEx(self.handle.load(Ordering::SeqCst) as HANDLE, std::ptr::null());
        }
    }
}

unsafe fn any_relevant(mut p: *const u8) -> bool {
    loop {
        let info = &*(p as *const FILE_NOTIFY_INFORMATION);
        let len = info.FileNameLength as usize / 2;
        let name = std::slice::from_raw_parts(info.FileName.as_ptr(), len);
        let s = String::from_utf16_lossy(name).replace('\\', "/");
        if is_relevant(&s) {
            return true;
        }
        if info.NextEntryOffset == 0 {
            return false;
        }
        p = p.add(info.NextEntryOffset as usize);
    }
}

pub fn is_relevant(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".lock") {
        return false;
    }
    if p == ".git" {
        return false;
    }
    if let Some(inner) = p.strip_prefix(".git/") {
        return inner == "index" || inner == "head" || inner.starts_with("refs") || inner == "packed-refs" || inner == "merge_head" || inner.starts_with("rebase-") || inner == "fetch_head" || inner == "cherry_pick_head" || inner == "stash";
    }
    !(p.starts_with("node_modules/") || p.contains("/node_modules/"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn relevance() {
        assert!(super::is_relevant("src/main.rs"));
        assert!(super::is_relevant(".git/index"));
        assert!(!super::is_relevant(".git/index.lock"));
        assert!(!super::is_relevant(".git/objects/ab/cdef"));
        assert!(super::is_relevant(".git/refs/heads/main"));
        assert!(!super::is_relevant("node_modules/x/y.js"));
    }
}

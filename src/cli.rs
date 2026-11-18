//! Command line handling, modeled on `code.exe`:
//!
//!   gitgui [options] [paths...]
//!
//! A path may be a repository folder, any folder inside it, its `.git` folder,
//! or a file inside it (the file's diff is opened). Relative paths such as `.`
//! are resolved against the current directory. Each additional path opens in
//! its own window.

use std::path::{Path, PathBuf};

#[derive(Default, Debug)]
pub struct Cli {
    pub paths: Vec<String>,
    pub help: bool,
    pub version: bool,
    pub install_menu: bool,
    pub uninstall_menu: bool,
    pub unknown: Vec<String>,
}

pub const USAGE: &str = "GitGui - a native, ultra-light Git client for Windows

Usage: gitgui.exe [options] [paths...]

  [paths...]                   A Git repository folder, any folder or file inside one,
                               or its .git folder. Relative paths (e.g. \".\") are
                               resolved against the current directory. A file opens
                               its diff. Each extra path opens in a new window.

Options:
  -h, --help                   Print usage.
  -v, --version                Print version.
  -n, --new-window             Accepted for compatibility (every path gets a window).
  --install-context-menu       Add \"Open with GitGui\" to the Explorer folder menu.
  --uninstall-context-menu     Remove the Explorer folder menu entry.
";

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Cli {
    let mut c = Cli::default();
    let mut only_paths = false;
    for a in args {
        if !only_paths && a == "--" {
            only_paths = true;
            continue;
        }
        if !only_paths && (a.starts_with("--") || (a.starts_with('-') && a.len() == 2) || (a.starts_with('/') && a.len() == 2 && a != "/.")) {
            match a.as_str() {
                "-h" | "--help" | "/?" | "/h" => c.help = true,
                "-v" | "--version" => c.version = true,
                "-n" | "--new-window" | "-r" | "--reuse-window" => {}
                "--install-context-menu" => c.install_menu = true,
                "--uninstall-context-menu" => c.uninstall_menu = true,
                _ => c.unknown.push(a),
            }
            continue;
        }
        let p = clean_arg(&a);
        if !p.is_empty() {
            c.paths.push(p);
        }
    }
    c
}

/// Fixes common Windows quoting artifacts, e.g. `"C:\repo\"` arrives as `C:\repo"`.
pub fn clean_arg(a: &str) -> String {
    let s = a.trim().trim_matches('"').trim();
    let s = s.strip_prefix("file:///").map(|x| x.replace('/', "\\")).unwrap_or_else(|| s.to_string());
    s.replace("%20", " ")
}

/// Strips the `\\?\` verbatim prefix produced by `canonicalize`.
fn simplify(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    p
}

/// Resolved target of a command line path.
#[derive(Debug, PartialEq)]
pub struct Target {
    /// Folder to open (the repository is discovered from it).
    pub folder: PathBuf,
    /// File inside the folder whose changes should be shown, if a file was given.
    pub file: Option<PathBuf>,
}

pub fn resolve(arg: &str, cwd: &Path) -> Result<Target, String> {
    let raw = PathBuf::from(arg);
    let abs = if raw.is_absolute() { raw } else { cwd.join(raw) };
    let abs = std::fs::canonicalize(&abs).map(simplify).map_err(|_| format!("Path '{}' does not exist.", abs.display()))?;
    if abs.is_file() {
        let folder = abs.parent().map(Path::to_path_buf).unwrap_or_else(|| abs.clone());
        return Ok(Target { folder, file: Some(abs) });
    }
    // the .git folder itself (or something inside it) -> its working tree
    let mut cur: Option<&Path> = Some(&abs);
    while let Some(p) = cur {
        if p.file_name().map(|n| n.eq_ignore_ascii_case(".git")).unwrap_or(false) {
            if let Some(parent) = p.parent() {
                return Ok(Target { folder: parent.to_path_buf(), file: None });
            }
        }
        cur = p.parent();
    }
    Ok(Target { folder: abs, file: None })
}

// ------------------------------------------------------------ console output

/// Writes to the console of the parent process (when started from a terminal);
/// returns false when there is none (e.g. started from Explorer).
pub fn print_to_parent_console(text: &str) -> bool {
    use std::io::Write;
    use windows_sys::Win32::System::Console::{AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_OUTPUT_HANDLE};
    unsafe {
        // redirected / piped stdout (e.g. `gitgui --version > v.txt`, Git Bash, CI)
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        if !h.is_null() && h != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            let mut out = std::io::stdout();
            if out.write_all(text.as_bytes()).is_ok() && out.flush().is_ok() {
                return true;
            }
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return false;
        }
    }
    match std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        Ok(mut f) => {
            let _ = f.write_all(format!("\r\n{}", text.replace('\n', "\r\n")).as_bytes());
            true
        }
        Err(_) => false,
    }
}

pub fn message_box(title: &str, text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
    let t = crate::win::wz(title);
    let m = crate::win::wz(text);
    unsafe {
        MessageBoxW(std::ptr::null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

pub fn output(title: &str, text: &str) {
    if !print_to_parent_console(text) {
        message_box(title, text);
    }
}

// --------------------------------------------------------- Explorer context menu

fn set_reg(key: &str, value: Option<&str>, data: &str) -> bool {
    use windows_sys::Win32::System::Registry::{RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ};
    let k = crate::win::wz(key);
    let v = value.map(crate::win::wz);
    let d = crate::win::wz(data);
    unsafe {
        let mut h: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut()) != 0 {
            return false;
        }
        let r = RegSetValueExW(h, v.as_ref().map(|x| x.as_ptr()).unwrap_or(std::ptr::null()), 0, REG_SZ, d.as_ptr() as *const u8, (d.len() * 2) as u32);
        RegCloseKey(h);
        r == 0
    }
}

const MENU_KEYS: [(&str, &str); 2] = [(r"Software\Classes\Directory\shell\GitGui", "%V"), (r"Software\Classes\Directory\Background\shell\GitGui", "%V")];

pub fn install_context_menu() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.display().to_string();
    for (key, arg) in MENU_KEYS {
        let ok = set_reg(key, None, "Open with GitGui") && set_reg(key, Some("Icon"), &exe) && set_reg(&format!(r"{key}\command"), None, &format!("\"{exe}\" \"{arg}\""));
        if !ok {
            return Err(format!("Unable to write registry key HKCU\\{key}"));
        }
    }
    Ok(())
}

pub fn uninstall_context_menu() {
    use windows_sys::Win32::System::Registry::{RegDeleteTreeW, HKEY_CURRENT_USER};
    for (key, _) in MENU_KEYS {
        let k = crate::win::wz(key);
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_flags_and_paths() {
        let c = parse(args(&["-n", ".", "--", "-weird-name", "--help"]));
        assert_eq!(c.paths, vec![".", "-weird-name", "--help"]);
        assert!(!c.help);
        let c = parse(args(&["--version", "C:\\repo\""]));
        assert!(c.version);
        assert_eq!(c.paths, vec!["C:\\repo\\"[..7].to_string()]);
    }

    #[test]
    fn resolves_relative_git_dir_and_files() {
        let root = std::env::temp_dir().join(format!("gitgui-cli-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git").join("refs")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src").join("a.txt"), "x").unwrap();
        let canon = simplify(std::fs::canonicalize(&root).unwrap());

        assert_eq!(resolve(".", &root).unwrap(), Target { folder: canon.clone(), file: None });
        assert_eq!(resolve("src", &root).unwrap().folder, canon.join("src"));
        assert_eq!(resolve(".git", &root).unwrap(), Target { folder: canon.clone(), file: None });
        assert_eq!(resolve(".git\\refs", &root).unwrap().folder, canon);
        let t = resolve("src\\a.txt", &root).unwrap();
        assert_eq!(t.folder, canon.join("src"));
        assert_eq!(t.file, Some(canon.join("src").join("a.txt")));
        assert!(resolve("missing", &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}

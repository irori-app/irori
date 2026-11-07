//! Windows files the user opens with Irori from the file manager (`.exe`,
//! `.msi`, `.bat`, `.cmd`, `.reg`): which files count, and the Wine command
//! that opens each one in a game's prefix.

use std::path::Path;

/// File name extensions Irori opens, lower case.
pub const EXTENSIONS: &[&str] = &["exe", "msi", "bat", "cmd", "reg"];

/// Whether the path names a Windows file Irori opens, by its extension
/// (any letter case). Reads nothing from disk.
pub fn is_windows_file(path: &Path) -> bool {
    extension(path).is_some()
}

/// The command that opens the file in a Wine prefix, program first.
///
/// Wine starts anything that isn't a program through its `start`, which
/// opens batch files with `cmd` and installer packages with `msiexec`.
/// Registry files go to `regedit` themselves: a new prefix has no command
/// for opening them, so `start` refuses them.
pub fn command(path: &Path) -> Option<Vec<String>> {
    let file = path.to_string_lossy().into_owned();
    Some(match extension(path)? {
        "reg" => vec!["regedit".into(), file],
        _ => vec![file],
    })
}

fn extension(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?;
    EXTENSIONS
        .iter()
        .copied()
        .find(|e| e.eq_ignore_ascii_case(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_windows_files_by_extension() {
        assert!(is_windows_file(Path::new("/games/Setup.EXE")));
        assert!(is_windows_file(Path::new("/games/patch.msi")));
        assert!(is_windows_file(Path::new("run.Bat")));
        assert!(is_windows_file(Path::new("fix.cmd")));
        assert!(is_windows_file(Path::new("tweak.reg")));
        assert!(!is_windows_file(Path::new("/games/readme.txt")));
        assert!(!is_windows_file(Path::new("/games/exe")));
        assert!(!is_windows_file(Path::new("/games/.exe.txt")));
    }

    #[test]
    fn registry_files_open_with_regedit() {
        let argv = |p: &str| command(Path::new(p)).unwrap();
        assert_eq!(argv("/g/setup.exe"), ["/g/setup.exe"]);
        assert_eq!(argv("/g/start.BAT"), ["/g/start.BAT"]);
        assert_eq!(argv("/g/patch.MSI"), ["/g/patch.MSI"]);
        assert_eq!(argv("/g/fix.reg"), ["regedit", "/g/fix.reg"]);
        assert_eq!(command(Path::new("/g/notes.txt")), None);
    }
}

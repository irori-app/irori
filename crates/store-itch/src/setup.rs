//! Windows setup programs uploaded to itch.io: thin re-export of
//! [`irori_runtime::setup`].

pub(crate) use irori_runtime::setup::{Setup, command, detect};

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn reexports_runtime_setup_detection_and_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let inno = tmp.path().join("setup.exe");
        std::fs::write(&inno, [b"MZ\x90\x00".as_slice(), b"...Inno Setup"].concat()).unwrap();
        assert_eq!(detect(&inno, "setup.exe"), Some(Setup::Inno));

        let unins = tmp.path().join("unins000.exe");
        std::fs::write(
            &unins,
            [b"MZ\x90\x00".as_slice(), b"...Inno Setup"].concat(),
        )
        .unwrap();
        assert_eq!(detect(&unins, "unins000.exe"), None);

        let argv = command(
            Setup::Nsis,
            Path::new(r"C:\dl\setup.exe"),
            Path::new(r"C:\Games\My Game"),
            Path::new(r"C:\dl\setup.log"),
            true,
        );
        assert_eq!(argv.last().unwrap(), r"/D=C:\Games\My Game");
    }
}

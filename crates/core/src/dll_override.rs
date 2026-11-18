//! Wine DLL overrides (`iroriDllOverrides`): which copy of a DLL Wine loads
//! for a game, the game folder's (native) or Wine's own (builtin), or none.
//! Launches turn them into `WINEDLLOVERRIDES`; Lutris scripts and imports
//! carry them as `wine: overrides:`, whose spelling ([`DllMode::parse`])
//! is accepted here too.

use serde::{Deserialize, Serialize};

/// One override: a DLL and the mode Wine loads it in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DllOverride {
    /// The DLL's name, as `dxgi` or `powershell.exe`.
    pub dll: String,
    /// Wine's spelling of the mode (`n`, `b`, `n,b`, `b,n` or `d`); see
    /// [`DllMode`].
    pub mode: String,
}

/// How Wine loads a DLL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DllMode {
    Native,
    Builtin,
    NativeThenBuiltin,
    BuiltinThenNative,
    Disabled,
}

impl DllMode {
    pub const ALL: [DllMode; 5] = [
        DllMode::NativeThenBuiltin,
        DllMode::BuiltinThenNative,
        DllMode::Native,
        DllMode::Builtin,
        DllMode::Disabled,
    ];

    /// Read a mode as Wine (`n,b`, `d`, empty for disabled) or Lutris
    /// (`native`, `builtin`, `disabled`, with spaces) write it.
    pub fn parse(text: &str) -> Option<Self> {
        let text: String = text
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        let parts: Vec<&str> = text.split(',').collect();
        let one = |p: &str| match p {
            "n" | "native" => Some(DllMode::Native),
            "b" | "builtin" => Some(DllMode::Builtin),
            _ => None,
        };
        match parts.as_slice() {
            [""] | ["d"] | ["disabled"] | ["disable"] => Some(DllMode::Disabled),
            [p] => one(p),
            [a, b] => match (one(a)?, one(b)?) {
                (DllMode::Native, DllMode::Builtin) => Some(DllMode::NativeThenBuiltin),
                (DllMode::Builtin, DllMode::Native) => Some(DllMode::BuiltinThenNative),
                _ => None,
            },
            _ => None,
        }
    }

    /// The mode as `WINEDLLOVERRIDES` writes it.
    pub fn wine_value(self) -> &'static str {
        match self {
            DllMode::Native => "n",
            DllMode::Builtin => "b",
            DllMode::NativeThenBuiltin => "n,b",
            DllMode::BuiltinThenNative => "b,n",
            DllMode::Disabled => "d",
        }
    }
}

/// A DLL name as written (`d3d11`, `D3D11.dll`, `powershell.exe`), trimmed;
/// `None` when it can't be one: empty, or holding a separator Wine reads
/// (`=`, `;`, `,`), a blank or a path.
pub fn dll_name(text: &str) -> Option<String> {
    let name = text.trim();
    let bad = |c: char| c.is_whitespace() || matches!(c, '=' | ';' | ',' | '/' | '\\' | '"');
    (!name.is_empty() && !name.contains(bad)).then(|| name.to_string())
}

impl DllOverride {
    /// The override for `dll` in `mode`, both checked and `mode` in Wine's
    /// spelling.
    pub fn new(dll: &str, mode: &str) -> Option<Self> {
        Some(Self {
            dll: dll_name(dll)?,
            mode: DllMode::parse(mode)?.wine_value().to_string(),
        })
    }

    /// Overrides for `dlls`, one or several names separated by commas as
    /// Lutris and Wine allow (`nvapi,nvapi64`), all in `mode`. `None` when a
    /// name or the mode can't be read.
    pub fn several(dlls: &str, mode: &str) -> Option<Vec<Self>> {
        dlls.split(',')
            .filter(|d| !d.trim().is_empty())
            .map(|dll| Self::new(dll, mode))
            .collect::<Option<Vec<_>>>()
            .filter(|list| !list.is_empty())
    }

    /// `(dll, mode)` for `WINEDLLOVERRIDES`; `None` for an entry that can't
    /// be read (left out of the launch rather than breaking the variable).
    pub fn wine_pair(&self) -> Option<(String, &'static str)> {
        Some((
            dll_name(&self.dll)?,
            DllMode::parse(&self.mode)?.wine_value(),
        ))
    }
}

/// Add `add` to `list`, each replacing an entry for the same DLL (names
/// compare without letter case or `.dll`).
pub fn merge(list: &mut Vec<DllOverride>, add: impl IntoIterator<Item = DllOverride>) {
    let key = |name: &str| {
        let name = name.trim().to_ascii_lowercase();
        name.strip_suffix(".dll").map(str::to_owned).unwrap_or(name)
    };
    for o in add {
        match list.iter_mut().find(|e| key(&e.dll) == key(&o.dll)) {
            Some(existing) => *existing = o,
            None => list.push(o),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_read_wine_and_lutris_spellings() {
        for (text, mode) in [
            ("n", DllMode::Native),
            ("native", DllMode::Native),
            ("B", DllMode::Builtin),
            ("n,b", DllMode::NativeThenBuiltin),
            ("native, builtin", DllMode::NativeThenBuiltin),
            ("builtin,native", DllMode::BuiltinThenNative),
            ("disabled", DllMode::Disabled),
            ("d", DllMode::Disabled),
            ("", DllMode::Disabled),
        ] {
            assert_eq!(DllMode::parse(text), Some(mode), "{text}");
        }
        for text in ["x", "n,n", "n,b,d", "native;builtin"] {
            assert_eq!(DllMode::parse(text), None, "{text}");
        }
    }

    #[test]
    fn names_refuse_what_would_break_the_variable() {
        assert_eq!(dll_name(" d3d11.dll ").as_deref(), Some("d3d11.dll"));
        assert_eq!(
            dll_name("powershell.exe").as_deref(),
            Some("powershell.exe")
        );
        for bad in ["", " ", "a=b", "a;b", "a,b", "a b", "C:\\x.dll", "../x"] {
            assert_eq!(dll_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn several_names_share_a_mode() {
        let list = DllOverride::several("nvapi,nvapi64", "disabled").unwrap();
        assert_eq!(
            list,
            [
                DllOverride::new("nvapi", "d").unwrap(),
                DllOverride::new("nvapi64", "d").unwrap()
            ]
        );
        assert_eq!(DllOverride::several("nvapi", "maybe"), None);
        assert_eq!(DllOverride::several("", "n"), None);
    }

    #[test]
    fn merging_replaces_the_same_dll() {
        let mut list = vec![
            DllOverride::new("dxgi", "n").unwrap(),
            DllOverride::new("d3d9", "b").unwrap(),
        ];
        merge(
            &mut list,
            [
                DllOverride::new("DXGI.dll", "b").unwrap(),
                DllOverride::new("xinput1_3", "n,b").unwrap(),
            ],
        );
        let pairs: Vec<_> = list.iter().filter_map(DllOverride::wine_pair).collect();
        assert_eq!(
            pairs,
            [
                ("DXGI.dll".to_string(), "b"),
                ("d3d9".to_string(), "b"),
                ("xinput1_3".to_string(), "n,b")
            ]
        );
        let broken = DllOverride {
            dll: "a b".into(),
            mode: "n".into(),
        };
        assert_eq!(broken.wine_pair(), None);
    }
}

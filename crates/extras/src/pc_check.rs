//! "Can my PC run it?": a store page's minimum and recommended system
//! requirements read into memory, graphics card and processor needs, and
//! compared with this computer's.
//!
//! Only memory compares exactly. Graphics cards and processors are ranked
//! by a rough table of the common desktop and laptop families
//! ([`gpu_score`], [`cpu_score`]); a name the table doesn't know, on
//! either side, is "can't tell", never a guess, and the verdict says so.
//! Requirements are read in any of Irori's languages, since Steam sends
//! them in the interface language.

use irori_core::Requirement;
use once_cell::sync::Lazy;
use regex::Regex;

/// What this computer has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThisPc {
    pub memory_bytes: u64,
    /// The processor's name as the system gives it.
    pub cpu: String,
    /// Logical processors.
    pub cpu_threads: usize,
    /// The graphics cards' names; empty when the system doesn't say.
    pub gpus: Vec<String>,
}

/// A part a game needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartKind {
    Memory,
    Graphics,
    Processor,
}

/// How this computer compares with one level of a part's need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Meets,
    Short,
    /// One side couldn't be read or ranked.
    Unknown,
    /// The page names no need at this level.
    NotGiven,
}

/// One part's comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartCheck {
    pub kind: PartKind,
    /// What the page asks for, as written.
    pub minimum: Option<String>,
    pub recommended: Option<String>,
    /// What this computer has, as shown to the user.
    pub have: Option<String>,
    pub at_minimum: Fit,
    pub at_recommended: Fit,
}

/// The overall answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Every part that could be compared meets the recommended needs.
    Recommended,
    /// Every part that could be compared meets the minimum.
    Minimum,
    /// A part is below the minimum.
    Below,
    /// Nothing could be compared.
    Unknown,
}

/// A whole comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcCheck {
    pub verdict: Verdict,
    /// Whether a part couldn't be compared, so the verdict is partial.
    pub partial: bool,
    /// The system whose requirements were read (`Windows`, `Linux`…), when
    /// the page names one.
    pub system: Option<String>,
    pub parts: Vec<PartCheck>,
}

/// Words that name a part in a requirement's label, in Irori's languages.
const LABELS: &[(PartKind, &[&str])] = &[
    (
        PartKind::Memory,
        &[
            "memory",
            "ram",
            "arbeitsspeicher",
            "mémoire",
            "memoire",
            "memoria",
            "geheugen",
            "pamięć",
            "pamiec",
            "память",
            "пам'ять",
            "памʼять",
            "メモリ",
            "메모리",
            "内存",
            "記憶體",
            "bellek",
            "minne",
            "paměť",
            "pamet",
            "memorie",
        ],
    ),
    (
        PartKind::Graphics,
        &[
            "graphics",
            "video",
            "gpu",
            "grafik",
            "graphique",
            "gráfica",
            "grafica",
            "videokaart",
            "graficzna",
            "видеокарта",
            "відеокарта",
            "グラフィック",
            "ビデオ",
            "그래픽",
            "显卡",
            "顯示卡",
            "显示卡",
            "ekran kartı",
            "grafikkort",
            "grafika",
            "placă video",
        ],
    ),
    (
        PartKind::Processor,
        &[
            "processor",
            "cpu",
            "prozessor",
            "processeur",
            "procesador",
            "processore",
            "procesor",
            "processador",
            "процессор",
            "процесор",
            "プロセッサ",
            "프로세서",
            "处理器",
            "處理器",
            "işlemci",
            "procesorul",
        ],
    ),
];

/// The part a requirement label names.
fn part_of(label: &str) -> Option<PartKind> {
    let label = label.to_lowercase();
    // "Video memory" is the card's, not the computer's.
    if ["vram", "video memory", "videospeicher", "mémoire vidéo"]
        .iter()
        .any(|w| label.contains(w))
    {
        return Some(PartKind::Graphics);
    }
    LABELS
        .iter()
        .find(|(_, words)| {
            words.iter().any(|w| {
                // Short words must be whole ("ram" isn't in "program").
                if w.len() <= 3 {
                    label
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|part| part == *w)
                } else {
                    label.contains(w)
                }
            })
        })
        .map(|(kind, _)| *kind)
}

/// The `label: value` lines of a requirement text; a row whose text has
/// none is the row's own title and text.
fn fields<'a>(title: &'a str, text: &'a str) -> Vec<(&'a str, &'a str)> {
    let lines: Vec<(&str, &str)> = text
        .lines()
        .filter_map(|l| {
            let (label, value) = l.split_once([':', '：'])?;
            let value = value.trim();
            (!value.is_empty()).then_some((label.trim(), value))
        })
        .collect();
    if lines.is_empty() && !text.trim().is_empty() {
        vec![(title, text.trim())]
    } else {
        lines
    }
}

/// Operating systems a requirements row can be the whole set of.
fn is_system(title: &str) -> bool {
    let t = title.trim().to_lowercase();
    [
        "windows", "linux", "macos", "mac os", "osx", "mac", "steamos", "pc",
    ]
    .iter()
    .any(|s| t == *s || t.starts_with(&format!("{s} ")))
}

/// A part and what the page asks of it at the minimum and recommended
/// levels.
type Need = (PartKind, Option<String>, Option<String>);

/// Each part's need at the minimum and the recommended level, from the
/// rows of one system (the first, which is this computer's when the page
/// has several) or from per-part rows; with that system's name.
fn needs(rows: &[Requirement]) -> (Option<String>, Vec<Need>) {
    let (system, rows): (Option<String>, &[Requirement]) = match rows.first() {
        Some(first) if is_system(&first.title) => (
            Some(first.title.trim().to_string()),
            std::slice::from_ref(first),
        ),
        _ => (None, rows),
    };
    let mut out: Vec<Need> = Vec::new();
    for row in rows {
        for (level, text) in [(0, &row.minimum), (1, &row.recommended)] {
            for (label, value) in fields(&row.title, text) {
                let Some(kind) = part_of(label) else {
                    continue;
                };
                let slot = match out.iter_mut().find(|(k, _, _)| *k == kind) {
                    Some(slot) => slot,
                    None => {
                        out.push((kind, None, None));
                        out.last_mut().expect("just pushed")
                    }
                };
                let target = if level == 0 { &mut slot.1 } else { &mut slot.2 };
                if target.is_none() {
                    *target = Some(value.to_string());
                }
            }
        }
    }
    out.sort_by_key(|(k, _, _)| *k as u8);
    (system, out)
}

static AMOUNT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(\d+(?:[.,]\d+)?)\s*(tb|gb|gib|go|гб|g|mb|mib|mo|мб)\b").expect("regex")
});

/// The memory a text names, in MiB (`8 GB RAM` → 8192).
pub fn memory_mib(text: &str) -> Option<u64> {
    let caps = AMOUNT.captures(text)?;
    let number: f64 = caps[1].replace(',', ".").parse().ok()?;
    let unit = caps[2].to_lowercase();
    let mib = match unit.as_str() {
        "tb" => number * 1024.0 * 1024.0,
        "mb" | "mib" | "mo" | "мб" => number,
        _ => number * 1024.0,
    };
    (mib >= 1.0).then_some(mib.round() as u64)
}

/// Rough relative speed of graphics cards (higher is faster), from common
/// benchmark results of each model; only to tell a card above or below
/// another of a quite different class. Each entry is a model's words, in
/// the order its name has them.
const GPUS: &[(&str, u32)] = &[
    // NVIDIA, newest first so "rtx 4060 ti" wins over "rtx 4060".
    ("rtx 5090", 400),
    ("rtx 5080", 360),
    ("rtx 5070 ti", 330),
    ("rtx 5070", 300),
    ("rtx 5060 ti", 250),
    ("rtx 5060", 230),
    ("rtx 4090", 385),
    ("rtx 4080 super", 350),
    ("rtx 4080", 345),
    ("rtx 4070 ti super", 320),
    ("rtx 4070 ti", 315),
    ("rtx 4070 super", 300),
    ("rtx 4070", 268),
    ("rtx 4060 ti", 225),
    ("rtx 4060", 198),
    ("rtx 4050", 160),
    ("rtx 3090 ti", 280),
    ("rtx 3090", 265),
    ("rtx 3080 ti", 268),
    ("rtx 3080", 250),
    ("rtx 3070 ti", 235),
    ("rtx 3070", 223),
    ("rtx 3060 ti", 200),
    ("rtx 3060", 170),
    ("rtx 3050", 128),
    ("rtx 2080 ti", 218),
    ("rtx 2080 super", 200),
    ("rtx 2080", 190),
    ("rtx 2070 super", 182),
    ("rtx 2070", 162),
    ("rtx 2060 super", 160),
    ("rtx 2060", 140),
    ("gtx 1660 ti", 128),
    ("gtx 1660 super", 127),
    ("gtx 1660", 115),
    ("gtx 1650 super", 100),
    ("gtx 1650", 78),
    ("gtx 1630", 55),
    ("gtx 1080 ti", 185),
    ("gtx 1080", 154),
    ("gtx 1070 ti", 145),
    ("gtx 1070", 134),
    ("gtx 1060", 100),
    ("gtx 1050 ti", 63),
    ("gtx 1050", 49),
    ("gt 1030", 25),
    ("titan x", 135),
    ("gtx 980 ti", 135),
    ("gtx 980", 110),
    ("gtx 970", 96),
    ("gtx 960", 60),
    ("gtx 950", 53),
    ("gtx 780 ti", 85),
    ("gtx 780", 75),
    ("gtx 770", 60),
    ("gtx 760", 49),
    ("gtx 750 ti", 39),
    ("gtx 750", 33),
    ("gtx 690", 70),
    ("gtx 680", 58),
    ("gtx 670", 55),
    ("gtx 660 ti", 48),
    ("gtx 660", 41),
    ("gtx 650 ti", 29),
    ("gtx 650", 19),
    ("gtx 580", 40),
    ("gtx 570", 36),
    ("gtx 560 ti", 31),
    ("gtx 560", 27),
    ("gtx 550 ti", 19),
    ("gtx 480", 33),
    ("gtx 470", 28),
    ("gtx 460", 22),
    ("gt 730", 9),
    ("gt 640", 12),
    // AMD.
    ("rx 9070 xt", 310),
    ("rx 9070", 290),
    ("rx 9060 xt", 220),
    ("rx 7900 xtx", 310),
    ("rx 7900 xt", 285),
    ("rx 7900 gre", 260),
    ("rx 7800 xt", 250),
    ("rx 7700 xt", 220),
    ("rx 7600 xt", 170),
    ("rx 7600", 165),
    ("rx 6950 xt", 275),
    ("rx 6900 xt", 265),
    ("rx 6800 xt", 250),
    ("rx 6800", 220),
    ("rx 6750 xt", 198),
    ("rx 6700 xt", 190),
    ("rx 6700", 175),
    ("rx 6650 xt", 165),
    ("rx 6600 xt", 160),
    ("rx 6600", 150),
    ("rx 6500 xt", 75),
    ("rx 6400", 70),
    ("rx 5700 xt", 160),
    ("rx 5700", 140),
    ("rx 5600 xt", 130),
    ("rx 5500 xt", 95),
    ("radeon vii", 160),
    ("vega 64", 140),
    ("vega 56", 125),
    ("rx 590", 90),
    ("rx 580", 86),
    ("rx 570", 73),
    ("rx 560", 35),
    ("rx 550", 28),
    ("rx 480", 75),
    ("rx 470", 65),
    ("rx 460", 33),
    ("r9 fury x", 105),
    ("r9 fury", 95),
    ("r9 390x", 88),
    ("r9 390", 84),
    ("r9 380x", 58),
    ("r9 380", 53),
    ("r9 290x", 83),
    ("r9 290", 78),
    ("r9 280x", 56),
    ("r9 280", 50),
    ("r9 270x", 44),
    ("r9 270", 40),
    ("r7 370", 36),
    ("r7 265", 35),
    ("r7 260x", 30),
    ("r7 260", 26),
    ("hd 7970", 58),
    ("hd 7950", 52),
    ("hd 7870", 45),
    ("hd 7850", 38),
    ("hd 7790", 30),
    ("hd 7770", 22),
    ("hd 6970", 32),
    ("hd 6950", 30),
    ("hd 6870", 25),
    ("hd 6850", 22),
    ("hd 5870", 25),
    ("hd 5850", 21),
    ("radeon 890m", 85),
    ("radeon 880m", 75),
    ("radeon 780m", 65),
    ("radeon 760m", 50),
    ("radeon 680m", 58),
    ("radeon 660m", 38),
    // Intel.
    ("arc b580", 160),
    ("arc b570", 145),
    ("arc a770", 150),
    ("arc a750", 140),
    ("arc a580", 110),
    ("arc a380", 60),
    ("arc a310", 40),
    ("iris xe", 28),
    ("uhd 770", 15),
    ("uhd 750", 14),
    ("uhd 730", 13),
    ("uhd 630", 13),
    ("uhd 620", 9),
    ("hd 630", 13),
    ("hd 620", 9),
    ("hd 530", 11),
    ("hd 520", 8),
    ("hd 4600", 7),
    ("hd 4000", 5),
];

/// Lowercase words of a name, with a model number cut from the family
/// letters it is written against (`GTX1060` → `gtx 1060`, `1060Ti` →
/// `1060 ti`, while `i5`, `R9`, `A770` and `780M` stay whole) and the word
/// "graphics" left out (`UHD Graphics 620` → `uhd 620`).
fn words(name: &str) -> String {
    let lower = name
        .to_lowercase()
        .replace(['(', ')', '[', ']', '®', '™', '-', '_', ','], " ");
    let mut out: Vec<String> = Vec::new();
    for token in lower.split_whitespace() {
        if token == "graphics" {
            continue;
        }
        let mut runs: Vec<String> = Vec::new();
        for c in token.chars() {
            let same = runs.last().is_some_and(|r: &String| {
                r.chars()
                    .last()
                    .is_some_and(|l| l.is_ascii_digit() == c.is_ascii_digit())
            });
            match runs.last_mut() {
                Some(run) if same => run.push(c),
                _ => runs.push(c.to_string()),
            }
        }
        let mut word = String::new();
        for (i, run) in runs.iter().enumerate() {
            if i > 0 {
                let prev = &runs[i - 1];
                let digits = |r: &str| r.starts_with(|c: char| c.is_ascii_digit());
                let letters_long = |r: &str| !digits(r) && r.chars().count() >= 2;
                if (letters_long(prev) && digits(run)) || (digits(prev) && letters_long(run)) {
                    out.push(std::mem::take(&mut word));
                }
            }
            word.push_str(run);
        }
        out.push(word);
    }
    out.retain(|w| !w.is_empty());
    out.join(" ")
}

/// Whether `entry`'s words appear in `name`'s, in order and whole.
fn names_model(name: &str, entry: &str) -> bool {
    let name = format!(" {name} ");
    name.contains(&format!(" {entry} "))
}

/// The rough speed of the graphics card `name` names; `None` for one the
/// table doesn't know.
pub fn gpu_score(name: &str) -> Option<u32> {
    let name = words(name);
    // Laptop chips are slower than the desktop card of the same name.
    let mobile = ["laptop", "mobile", "max q", "max-q"]
        .iter()
        .any(|w| name.contains(w));
    GPUS.iter()
        .find(|(entry, _)| names_model(&name, entry))
        .map(|(_, score)| if mobile { score * 4 / 5 } else { *score })
}

/// The rough speed of a processor (higher is faster): Intel Core and AMD
/// Ryzen by tier and generation, older families low; `None` for one this
/// can't place (Apple's, other makers').
pub fn cpu_score(name: &str) -> Option<u32> {
    static CORE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bi([3579])\s*(\d{4,5})").expect("regex"));
    static ULTRA: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bcore ultra ([579])\b").expect("regex"));
    static RYZEN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\bryzen\s*(?:ai\s*)?([3579])\s*(?:pro\s*)?(\d)(\d{2,3})").expect("regex")
    });
    let name = words(name);
    let tier_value = |tier: u32| match tier {
        3 => 0,
        5 => 8,
        7 => 14,
        _ => 18,
    };
    if let Some(c) = ULTRA.captures(&name) {
        let tier: u32 = c[1].parse().ok()?;
        return Some(140 + tier_value(tier));
    }
    if let Some(c) = CORE.captures(&name) {
        let tier: u32 = c[1].parse().ok()?;
        let number = &c[2];
        let generation: u32 = if number.len() == 5 {
            number[..2].parse().ok()?
        } else {
            number[..1].parse().ok()?
        };
        return Some(generation * 10 + tier_value(tier));
    }
    if let Some(c) = RYZEN.captures(&name) {
        let tier: u32 = c[1].parse().ok()?;
        // Each Ryzen series next to the Intel generation it matches.
        let generation = match &c[2] {
            "1" => 7,
            "2" => 8,
            "3" | "4" => 10,
            "5" | "6" => 12,
            "7" | "8" => 13,
            "9" => 14,
            _ => return None,
        };
        return Some(generation * 10 + tier_value(tier));
    }
    let old = [
        ("fx ", 30),
        ("phenom", 15),
        ("athlon", 10),
        ("core 2 quad", 12),
        ("core 2 duo", 8),
        ("pentium", 12),
        ("celeron", 5),
    ];
    old.iter()
        .find(|(w, _)| format!("{name} ").contains(w))
        .map(|(_, s)| *s)
}

/// The weakest card or processor of the ones `text` lists as alternatives
/// that `score` can rank: the need is met by any of them.
fn needed_score(text: &str, score: fn(&str) -> Option<u32>) -> Option<u32> {
    text.split(['/', '|', ';', ',', '(', ')'])
        .flat_map(|part| part.split(" or "))
        .flat_map(|part| part.split(" oder "))
        .flat_map(|part| part.split(" ou "))
        .flat_map(|part| part.split(" o "))
        .filter_map(score)
        .min()
}

fn fit(have: Option<u32>, need: Option<&str>, score: fn(&str) -> Option<u32>) -> Fit {
    let Some(need) = need else {
        return Fit::NotGiven;
    };
    match (have, needed_score(need, score)) {
        (Some(have), Some(need)) if have >= need => Fit::Meets,
        (Some(_), Some(_)) => Fit::Short,
        _ => Fit::Unknown,
    }
}

/// Compare a store page's requirements with this computer.
pub fn check(rows: &[Requirement], pc: &ThisPc) -> Option<PcCheck> {
    let (system, needs) = needs(rows);
    if needs.is_empty() {
        return None;
    }
    let gib = |mib: u64| format!("{} GB", (mib + 512) / 1024);
    let best_gpu = pc.gpus.iter().filter_map(|g| gpu_score(g)).max();
    let parts: Vec<PartCheck> = needs
        .into_iter()
        .map(|(kind, minimum, recommended)| {
            let (have, at_minimum, at_recommended) = match kind {
                PartKind::Memory => {
                    let have = (pc.memory_bytes > 0).then_some(pc.memory_bytes / (1024 * 1024));
                    // A computer sold with 8 GB reports a little less.
                    let level = |need: Option<&String>| match need {
                        None => Fit::NotGiven,
                        Some(need) => match (have, memory_mib(need)) {
                            (Some(h), Some(n)) if h + 512 >= n => Fit::Meets,
                            (Some(_), Some(_)) => Fit::Short,
                            _ => Fit::Unknown,
                        },
                    };
                    (
                        have.map(gib),
                        level(minimum.as_ref()),
                        level(recommended.as_ref()),
                    )
                }
                PartKind::Graphics => (
                    pc.gpus.first().cloned(),
                    fit(best_gpu, minimum.as_deref(), gpu_score),
                    fit(best_gpu, recommended.as_deref(), gpu_score),
                ),
                PartKind::Processor => {
                    let have = cpu_score(&pc.cpu);
                    (
                        Some(pc.cpu.clone()).filter(|c| !c.trim().is_empty()),
                        fit(have, minimum.as_deref(), cpu_score),
                        fit(have, recommended.as_deref(), cpu_score),
                    )
                }
            };
            PartCheck {
                kind,
                minimum,
                recommended,
                have,
                at_minimum,
                at_recommended,
            }
        })
        .collect();
    Some(PcCheck {
        verdict: verdict(&parts),
        partial: parts
            .iter()
            .any(|p| p.at_minimum == Fit::Unknown || p.at_recommended == Fit::Unknown),
        system,
        parts,
    })
}

/// The answer over every part that could be compared: below when any is
/// below its minimum, recommended when every one meets what the page
/// recommends (and it recommends something), else the minimum.
fn verdict(parts: &[PartCheck]) -> Verdict {
    if parts.iter().any(|p| p.at_minimum == Fit::Short) {
        return Verdict::Below;
    }
    let known = |f: Fit| matches!(f, Fit::Meets | Fit::Short);
    let compared: Vec<&PartCheck> = parts
        .iter()
        .filter(|p| known(p.at_minimum) || known(p.at_recommended))
        .collect();
    if compared.is_empty() {
        return Verdict::Unknown;
    }
    let recommended = compared.iter().any(|p| p.at_recommended == Fit::Meets)
        && compared.iter().all(|p| {
            p.at_recommended == Fit::Meets
                || (p.at_recommended == Fit::NotGiven && p.at_minimum == Fit::Meets)
        });
    if recommended {
        Verdict::Recommended
    } else {
        Verdict::Minimum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steam_row(min: &str, rec: &str) -> Requirement {
        Requirement {
            title: "Windows".into(),
            minimum: min.into(),
            recommended: rec.into(),
        }
    }

    fn pc(memory_gib: u64, cpu: &str, gpu: &str) -> ThisPc {
        ThisPc {
            // What a machine sold with this much reports.
            memory_bytes: memory_gib * 1024 * 1024 * 1024 - 300 * 1024 * 1024,
            cpu: cpu.into(),
            cpu_threads: 12,
            gpus: vec![gpu.into()],
        }
    }

    #[test]
    fn reads_memory_amounts_in_any_language() {
        assert_eq!(memory_mib("8 GB RAM"), Some(8192));
        assert_eq!(memory_mib("16GB"), Some(16384));
        assert_eq!(memory_mib("4096 MB RAM"), Some(4096));
        assert_eq!(memory_mib("8 Go de mémoire vive"), Some(8192));
        assert_eq!(memory_mib("8 ГБ ОЗУ"), Some(8192));
        assert_eq!(memory_mib("1,5 GB"), Some(1536));
        assert_eq!(memory_mib("lots"), None);
    }

    #[test]
    fn model_numbers_stand_apart_from_family_letters() {
        assert_eq!(words("GTX1060 6GB"), "gtx 1060 6 gb");
        assert_eq!(words("RTX 4060Ti"), "rtx 4060 ti");
        assert_eq!(
            words("Intel(R) Core(TM) i5-4460"),
            "intel r core tm i5 4460"
        );
        assert_eq!(words("Radeon 780M"), "radeon 780m");
        assert_eq!(words("Intel UHD Graphics 620"), "intel uhd 620");
        assert_eq!(gpu_score("Intel UHD Graphics 620"), Some(9));
    }

    #[test]
    fn ranks_cards_and_processors_roughly() {
        assert!(gpu_score("NVIDIA GeForce GTX 960").unwrap() < gpu_score("GTX1070").unwrap());
        assert_eq!(gpu_score("GA104 [GeForce RTX 3070]"), Some(223));
        assert!(gpu_score("RTX 4060 Ti").unwrap() > gpu_score("RTX 4060").unwrap());
        assert!(gpu_score("GeForce RTX 3070 Laptop GPU").unwrap() < 223);
        assert_eq!(gpu_score("Navi 22 [Radeon RX 6700 XT]"), Some(190));
        assert_eq!(gpu_score("Apple M2"), None);
        assert_eq!(gpu_score("DirectX 11 compatible card"), None);
        assert!(
            cpu_score("Intel Core i5-4460").unwrap()
                < cpu_score("Intel(R) Core(TM) i7-8700K CPU").unwrap()
        );
        assert!(
            cpu_score("AMD Ryzen 5 1600").unwrap()
                < cpu_score("AMD Ryzen 7 5800X3D 8-Core").unwrap()
        );
        assert!(cpu_score("Intel Core i9-13900K").unwrap() > cpu_score("Ryzen 5 3600").unwrap());
        assert!(cpu_score("AMD FX-8350").unwrap() < cpu_score("i5-4460").unwrap());
        assert_eq!(cpu_score("Apple M2 Pro"), None);
    }

    #[test]
    fn compares_a_steam_page_with_this_computer() {
        let rows = [
            steam_row(
                "Minimum:\nOS: Windows 10\nProcessor: Intel Core i5-4460 or AMD FX-6300\nMemory: 8 GB RAM\nGraphics: NVIDIA GeForce GTX 960 2GB / AMD Radeon R9 280\nStorage: 50 GB available space",
                "Recommended:\nProcessor: Intel Core i7-8700K / AMD Ryzen 5 3600X\nMemory: 16 GB RAM\nGraphics: NVIDIA GeForce RTX 2070 / AMD Radeon RX 5700 XT",
            ),
            Requirement {
                title: "Linux".into(),
                minimum: "Memory: 64 GB RAM".into(),
                recommended: String::new(),
            },
        ];
        let strong = check(&rows, &pc(16, "AMD Ryzen 7 5800X3D", "RTX 3070")).unwrap();
        assert_eq!(strong.system.as_deref(), Some("Windows"));
        assert_eq!(strong.verdict, Verdict::Recommended);
        assert!(!strong.partial);
        let kinds: Vec<PartKind> = strong.parts.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [PartKind::Memory, PartKind::Graphics, PartKind::Processor]
        );

        let middle = check(&rows, &pc(8, "Intel Core i5-6600K", "GeForce GTX 1060 6GB")).unwrap();
        assert_eq!(middle.verdict, Verdict::Minimum);

        let weak = check(&rows, &pc(4, "Intel Core i5-6600K", "GTX 1060")).unwrap();
        assert_eq!(weak.verdict, Verdict::Below);
        assert_eq!(weak.parts[0].at_minimum, Fit::Short);
    }

    #[test]
    fn says_when_it_cannot_tell() {
        let rows = [steam_row(
            "Processor: 2 GHz dual core\nGraphics: DirectX 11 card with 1 GB",
            "",
        )];
        let mac = ThisPc {
            memory_bytes: 0,
            cpu: "Apple M2".into(),
            cpu_threads: 8,
            gpus: Vec::new(),
        };
        let answer = check(&rows, &mac).unwrap();
        assert_eq!(answer.verdict, Verdict::Unknown);
        assert!(answer.partial);
        // Memory known, the rest not: the answer is partial.
        let rows = [steam_row("Memory: 4 GB RAM\nGraphics: some card", "")];
        let answer = check(&rows, &pc(16, "Apple M2", "Apple M2")).unwrap();
        assert_eq!(answer.verdict, Verdict::Minimum);
        assert!(answer.partial);
        // No part named: nothing to say.
        assert!(check(&[steam_row("Storage: 4 GB", "")], &pc(16, "", "")).is_none());
    }

    #[test]
    fn reads_per_part_rows_in_other_languages() {
        // Epic and GOG give one row per part; Steam sends the interface
        // language.
        let rows = [
            Requirement {
                title: "Arbeitsspeicher".into(),
                minimum: "8 GB".into(),
                recommended: "16 GB".into(),
            },
            Requirement {
                title: "Grafikkarte".into(),
                minimum: "GeForce GTX 1050 Ti".into(),
                recommended: "GeForce RTX 2060".into(),
            },
            Requirement {
                title: "Programm".into(),
                minimum: "8 GB".into(),
                recommended: String::new(),
            },
        ];
        let answer = check(&rows, &pc(8, "Ryzen 5 3600", "Radeon RX 580")).unwrap();
        assert_eq!(answer.parts.len(), 2);
        assert_eq!(answer.verdict, Verdict::Minimum);
        assert_eq!(answer.parts[1].at_recommended, Fit::Short);
    }
}

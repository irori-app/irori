use std::io::Cursor;

use super::store::{self, ThemeRef};
use super::*;

/// A PNG of `w` × `h` pixels.
fn png(w: u32, h: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(w, h, image::Rgb([200, 100, 50]));
    let mut out = Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png encodes");
    out.into_inner()
}

/// A version 1 theme file called "Test" with `fields` added (or, for
/// `"name"`, replaced).
fn with(fields: &str) -> String {
    let name = if fields.starts_with(r#","name""#) {
        ""
    } else {
        r#","name":"Test""#
    };
    format!(r#"{{"format":"irori-bigpicture-theme","version":1{name}{fields}}}"#)
}

#[test]
fn colours_parse_in_both_lengths() {
    assert_eq!(Rgb::parse("#ff8800"), Some(Rgb([255, 136, 0])));
    assert_eq!(Rgb::parse("#F80"), Some(Rgb([255, 136, 0])));
    assert_eq!(Rgb::parse(" #0a0B0c "), Some(Rgb([10, 11, 12])));
    for bad in ["ff8800", "#ff88", "#gg0000", "#ff880000", "", "#", "#ééé"] {
        assert_eq!(Rgb::parse(bad), None, "{bad}");
    }
    assert_eq!(Rgb([1, 2, 255]).hex(), "#0102ff");
    let red = Rgb::from_hsl(0.0, 1.0, 0.5);
    assert_eq!(red, Rgb([255, 0, 0]));
}

#[test]
fn a_minimal_file_takes_the_defaults() {
    let theme = Theme::from_json(&with("")).unwrap();
    assert_eq!(
        theme,
        Theme {
            name: "Test".into(),
            ..Theme::default()
        }
    );
}

#[test]
fn a_full_theme_round_trips() {
    let theme = Theme {
        name: "Sunset “Deluxe”".into(),
        author: "Someone".into(),
        layout: Some(HomeLayout::Carousel),
        palette: Palette {
            background: Some(Rgb([1, 2, 3])),
            foreground: Some(Rgb([250, 250, 250])),
            accent: Some(Rgb([255, 120, 0])),
            surface: None,
        },
        font: FontChoice::Serif,
        background: Background::Picture(Picture::from_bytes(png(3, 2)).unwrap()),
        covers: CoverStyle {
            shape: CoverShape::Landscape,
            size: CoverSize::Large,
            corners: Corners::Round,
        },
        shelves: vec![ShelfKind::PlanToPlay, ShelfKind::Systems],
    };
    let json = theme.to_json();
    assert!(json.contains("data:image/png;base64,"));
    assert_eq!(Theme::from_json(&json).unwrap(), theme);

    let gradient = Theme {
        background: Background::Gradient {
            from: Rgb([0, 0, 0]),
            to: Rgb([9, 9, 9]),
            angle: 45,
        },
        shelves: Vec::new(),
        ..Theme::default()
    };
    assert_eq!(Theme::from_json(&gradient.to_json()).unwrap(), gradient);
}

#[test]
fn files_that_are_not_themes_are_refused() {
    for text in [
        "",
        "[]",
        "not json",
        r#"{"format":"something-else","version":1,"name":"x"}"#,
        r#"{"format":"irori-bigpicture-theme","version":0,"name":"x"}"#,
    ] {
        assert_eq!(Theme::from_json(text), Err(ThemeError::NotATheme), "{text}");
    }
    assert_eq!(
        Theme::from_json(r#"{"format":"irori-bigpicture-theme","version":7,"name":"x"}"#),
        Err(ThemeError::Newer(7))
    );
}

#[test]
fn every_field_is_checked() {
    let cases: &[(&str, ThemeError)] = &[
        (r#","name":"""#, ThemeError::Name),
        (r#","name":"   ""#, ThemeError::Name),
        (r#","name":"a\nb""#, ThemeError::Name),
        (
            &format!(r#","name":"{}""#, "x".repeat(49)),
            ThemeError::Name,
        ),
        (r#","author":"a\u0007""#, ThemeError::Author),
        (
            r#","colors":{"accent":"red"}"#,
            ThemeError::Color("colors.accent", "red".into()),
        ),
        (
            r#","layout":"tilted""#,
            ThemeError::Value("layout", "tilted".into()),
        ),
        (
            r#","font":"Comic""#,
            ThemeError::Value("font", "Comic".into()),
        ),
        (
            r#","covers":{"shape":"round"}"#,
            ThemeError::Value("covers.shape", "round".into()),
        ),
        (
            r#","shelves":["installed","everything"]"#,
            ThemeError::Value("shelves", "everything".into()),
        ),
        (
            r#","background":{"kind":"video"}"#,
            ThemeError::Value("background.kind", "video".into()),
        ),
        (
            r##","background":{"kind":"gradient","from":"#000","to":"#fff","angle":400}"##,
            ThemeError::Value("background.angle", "400".into()),
        ),
        (
            r##","background":{"kind":"gradient","from":"#000"}"##,
            ThemeError::Color("background.to", String::new()),
        ),
    ];
    for (fields, error) in cases {
        assert_eq!(
            Theme::from_json(&with(fields)).as_ref(),
            Err(error),
            "{fields}"
        );
    }
    // Shelves listed twice are kept once.
    let theme = Theme::from_json(&with(r#","shelves":["installed","installed"]"#)).unwrap();
    assert_eq!(theme.shelves, vec![ShelfKind::Installed]);
}

#[test]
fn pictures_must_be_embedded_images_within_limits() {
    let picture = |image: &str| {
        Theme::from_json(&with(&format!(
            r#","background":{{"kind":"picture","image":"{image}"}}"#
        )))
    };
    assert_eq!(
        Theme::from_json(&with(r#","background":{"kind":"picture"}"#)),
        Err(ThemeError::PictureNotEmbedded)
    );
    for outside in [
        "/home/me/wallpaper.png",
        "../../etc/passwd",
        "file:///etc/passwd",
        "https://example.com/a.png",
    ] {
        assert_eq!(
            picture(outside),
            Err(ThemeError::PictureNotEmbedded),
            "{outside}"
        );
    }
    assert_eq!(
        picture("data:image/svg+xml;base64,PHN2Zz4="),
        Err(ThemeError::PictureFormat)
    );
    // Says PNG, holds text.
    assert_eq!(
        picture(&format!("data:image/png;base64,{}", BASE64.encode("hello"))),
        Err(ThemeError::PictureFormat)
    );
    assert_eq!(
        picture("data:image/png;base64,***"),
        Err(ThemeError::PictureFormat)
    );
    let ok = picture(&format!(
        "data:image/png;base64,{}",
        BASE64.encode(png(4, 4))
    ));
    assert!(matches!(ok.unwrap().background, Background::Picture(_)));

    assert_eq!(
        Picture::from_bytes(png(MAX_PICTURE_SIDE + 1, 1)),
        Err(ThemeError::PictureTooLarge)
    );
    assert_eq!(
        Picture::from_bytes(vec![0; MAX_PICTURE_BYTES + 1]),
        Err(ThemeError::PictureTooLarge)
    );
    let huge = "A".repeat(MAX_PICTURE_BYTES / 3 * 4 + 64);
    assert_eq!(
        picture(&format!("data:image/png;base64,{huge}")),
        Err(ThemeError::PictureTooLarge)
    );
    assert_eq!(
        Theme::from_json(&" ".repeat(MAX_FILE_BYTES as usize + 1)),
        Err(ThemeError::TooLarge)
    );
}

#[test]
fn errors_name_what_is_wrong() {
    crate::register_catalogs();
    let message = ThemeError::Color("colors.accent", "red".into()).message();
    assert!(message.contains("colors.accent") && message.contains("red"));
    assert!(ThemeError::Newer(3).message().contains('3'));
}

#[test]
fn ids_are_safe_file_names() {
    for good in ["sunset", "neon-arcade-2", "a"] {
        assert!(store::is_valid_id(good), "{good}");
    }
    for bad in [
        "",
        "-a",
        "a-",
        "../x",
        "a/b",
        "a.b",
        "Sunset",
        "a b",
        "..",
        &"a".repeat(41),
    ] {
        assert!(!store::is_valid_id(bad), "{bad}");
    }
    assert_eq!(store::new_id("Neon Arcade!", []), "neon-arcade");
    assert_eq!(store::new_id("../../etc", []), "etc");
    assert_eq!(store::new_id("夜", []), "theme");
    assert_eq!(
        store::new_id("Moss", ["moss", "moss-2"].iter().copied()),
        "moss-3"
    );
    let long = store::new_id(&"word ".repeat(30), []);
    assert!(store::is_valid_id(&long), "{long}");
}

#[test]
fn theme_settings_never_point_outside_the_folder() {
    assert_eq!(ThemeRef::parse("user:moss"), ThemeRef::User("moss".into()));
    assert_eq!(
        ThemeRef::parse("builtin:noir"),
        ThemeRef::BuiltIn("noir".into())
    );
    for unknown in ["", "user:../x", "user:", "builtin:nope", "moss", "user:A"] {
        assert_eq!(
            ThemeRef::parse(unknown),
            ThemeRef::BuiltIn(gallery::DEFAULT_ID.into()),
            "{unknown}"
        );
    }
    for r in [
        ThemeRef::User("moss".into()),
        ThemeRef::BuiltIn("noir".into()),
        ThemeRef::BuiltIn(gallery::DEFAULT_ID.into()),
    ] {
        assert_eq!(ThemeRef::parse(&r.setting()), r);
    }
}

#[test]
fn themes_save_load_import_export_and_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("themes");
    assert!(store::load(&dir).is_empty());

    let theme = Theme {
        name: "Moss".into(),
        palette: Palette {
            accent: Some(Rgb([1, 200, 3])),
            ..Default::default()
        },
        ..Theme::default()
    };
    // Export adds the ending, and the file imports back as a copy.
    let exported = store::export(&theme, &tmp.path().join("shared")).unwrap();
    assert!(exported.ends_with("shared.irori-theme.json"));
    assert_eq!(store::export_name(&theme), "moss.irori-theme.json");
    let first = store::import(&dir, &exported).unwrap();
    let second = store::import(&dir, &exported).unwrap();
    assert_eq!((first.id.as_str(), second.id.as_str()), ("moss", "moss-2"));
    assert_eq!(first.theme, theme);

    // A broken file and a stray file are skipped.
    std::fs::write(dir.join("broken.irori-theme.json"), "{").unwrap();
    std::fs::write(dir.join("notes.txt"), "hi").unwrap();
    let loaded = store::load(&dir);
    assert_eq!(
        loaded.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(),
        ["moss", "moss-2"]
    );

    store::delete(&dir, "moss").unwrap();
    store::delete(&dir, "moss").unwrap();
    assert!(store::delete(&dir, "../themes").is_err());
    assert!(store::save(&dir, "../escape", &theme).is_err());
    assert!(!tmp.path().join("escape.irori-theme.json").exists());
    assert_eq!(store::load(&dir).len(), 1);

    // A file over the size limit is refused before it is read.
    let big = tmp.path().join("big.irori-theme.json");
    std::fs::write(&big, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
    let error = store::import(&dir, &big).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ThemeError>(),
        Some(&ThemeError::TooLarge)
    );
}

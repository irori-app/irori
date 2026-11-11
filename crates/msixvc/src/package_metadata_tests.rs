use super::*;
use std::io::Cursor;

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn fixture(names: &[&str]) -> (XvdFile, Vec<u8>, usize) {
    let len = 16 + 528 + 528 * names.len() + 4;
    let mut header = XvdHeader::buffer();
    header[512..520].copy_from_slice(b"msft-xvd");
    header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
    put32(&mut header, 652, len as u32);
    let header = XvdHeader::try_from_array(&header).unwrap();
    let layout = header.layout();
    let start = layout.user_data.start.to_bytes().0 as usize;
    assert_ne!(start, layout.user_data.start.0 as usize);
    let mut bytes = vec![0u8; start + len];
    put32(&mut bytes, start, 16);
    put32(&mut bytes, start + 4, 1);
    put32(&mut bytes, start + 16 + 524, names.len() as u32);
    let payload = 16 + 528 + 528 * names.len();
    for (i, name) in names.iter().enumerate() {
        let entry = start + 16 + 528 + 528 * i;
        for (j, unit) in name.encode_utf16().enumerate() {
            assert!(j < 260);
            bytes[entry + 2 * j..entry + 2 * j + 2].copy_from_slice(&unit.to_le_bytes());
        }
        put32(&mut bytes, entry + 520, 4);
        put32(&mut bytes, entry + 524, (payload - 16) as u32);
    }
    bytes[start + payload..start + payload + 4].copy_from_slice(b"test");
    (
        XvdFile {
            header,
            layout,
            encrypted_section_infos: vec![],
        },
        bytes,
        start,
    )
}

#[tokio::test]
async fn package_metadata_uses_byte_offsets_and_preserves_unicode() {
    let (xvd, bytes, start) = fixture(&["SegmentMetadata.bin", "dir/žluťoučký-🎮.json"]);
    let files = xvd
        .parse_user_package_files(Cursor::new(&bytes))
        .await
        .unwrap();
    assert_eq!(files.len(), 2);
    for name in ["SegmentMetadata.bin", "dir/žluťoučký-🎮.json"] {
        let file = &files[name];
        assert_eq!(file.offset, start as u64 + 16 + 528 + 2 * 528);
        assert_eq!(
            &bytes[file.offset as usize..file.offset as usize + file.length as usize],
            b"test"
        );
    }
}

#[tokio::test]
async fn package_metadata_empty_and_nonpackage_sections_stay_empty() {
    let (mut xvd, mut bytes, start) = fixture(&[]);
    assert!(
        xvd.parse_user_package_files(Cursor::new(&bytes))
            .await
            .unwrap()
            .is_empty()
    );
    put32(&mut bytes, start + 8, 7);
    assert!(
        xvd.parse_user_package_files(Cursor::new(&bytes))
            .await
            .unwrap()
            .is_empty()
    );
    xvd.layout.user_data.len = Bytes(0);
    assert!(
        xvd.parse_user_package_files(Cursor::new(Vec::<u8>::new()))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn package_metadata_rejects_out_of_section_headers_and_counts() {
    for length in [0, 15, u32::MAX] {
        let (xvd, mut bytes, start) = fixture(&["valid"]);
        put32(&mut bytes, start, length);
        assert!(
            xvd.parse_user_package_files(Cursor::new(bytes))
                .await
                .is_err()
        );
    }
    let (xvd, mut bytes, start) = fixture(&["valid"]);
    put32(&mut bytes, start + 16 + 524, u32::MAX);
    assert!(
        xvd.parse_user_package_files(Cursor::new(bytes))
            .await
            .is_err()
    );
    let (mut xvd, bytes, _) = fixture(&[]);
    xvd.layout.user_data.len = Bytes(15);
    assert!(
        xvd.parse_user_package_files(Cursor::new(bytes))
            .await
            .is_err()
    );
    let (mut xvd, bytes, _) = fixture(&[]);
    xvd.layout.user_data.len = Bytes(16 + 527);
    assert!(
        xvd.parse_user_package_files(Cursor::new(bytes))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn package_metadata_invalid_utf16_is_an_error_not_a_panic() {
    for unit in [0xd800u16, 0xdc00u16] {
        let (xvd, mut bytes, start) = fixture(&["a"]);
        bytes[start + 16 + 528..start + 16 + 528 + 2].copy_from_slice(&unit.to_le_bytes());
        let err = xvd
            .parse_user_package_files(Cursor::new(bytes))
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("UTF-16"));
    }
}

#[tokio::test]
async fn package_metadata_checks_payload_bounds_and_truncation() {
    for (size, offset) in [(u32::MAX, 0), (4, u32::MAX)] {
        let (xvd, mut bytes, start) = fixture(&["valid"]);
        put32(&mut bytes, start + 16 + 528 + 520, size);
        put32(&mut bytes, start + 16 + 528 + 524, offset);
        assert!(
            xvd.parse_user_package_files(Cursor::new(bytes))
                .await
                .is_err()
        );
    }
    let (xvd, mut bytes, start) = fixture(&["valid"]);
    bytes.truncate(start + 16 + 528 + 527);
    assert!(
        xvd.parse_user_package_files(Cursor::new(bytes))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn package_metadata_accepts_full_field_and_zero_length_at_section_end() {
    let name = "x".repeat(260);
    let (xvd, mut bytes, start) = fixture(&[&name]);
    let len = xvd.layout.user_data.len.0;
    put32(&mut bytes, start + 16 + 528 + 520, 0);
    put32(&mut bytes, start + 16 + 528 + 524, (len - 16) as u32);
    let files = xvd
        .parse_user_package_files(Cursor::new(bytes))
        .await
        .unwrap();
    assert_eq!(files[&name].length, 0);
    assert_eq!(files[&name].offset, start as u64 + len);
}

// Hardware probing utilities

use std::io;

use base64_23::prelude::*;
#[cfg(any(target_os = "macos", target_os = "ios", target_family = "windows"))]
use smbioslib::raw_smbios_from_device;
#[cfg(not(target_os = "linux"))]
use smbioslib::{SMBiosSystemInformation, SystemUuidData, table_load_from_device};

use crate::clep;
use crate::models::devicecredential::Component;

pub fn probe_provision_components() -> Vec<Component> {
    let mut components = Vec::with_capacity(16);
    let drive_serial = [0u8];
    let mut smbios_buf = [0; 256];
    let mut drive_buf = [0; 64];

    let smbios = load_raw_smbios().ok();
    let parsed_smbios = load_smbios_fields(smbios.as_deref()).ok();

    drive_buf
        .iter_mut()
        .zip(drive_serial.iter())
        .for_each(|(place, data)| *place = *data);
    if let Some(smbios) = smbios.as_ref() {
        smbios_buf
            .iter_mut()
            .zip(smbios.iter())
            .for_each(|(place, data)| *place = *data);
    }
    let (clepv2, clepv4) = clep::challenge::get_license_challange(smbios_buf, drive_buf);

    components.push(Component::new(4113, "AA==".to_string()));
    components.push(Component::error(4101));
    components.push(Component::new(8196, BASE64_STANDARD.encode(clepv2)));
    components.push(Component::new(8197, BASE64_STANDARD.encode(clepv4)));

    if let Some((version, serial, uuid)) = parsed_smbios {
        components.push(Component::new(4100, BASE64_STANDARD.encode(version)));
        components.push(Component::new(4101, BASE64_STANDARD.encode(serial)));
        components.push(Component::new(4102, BASE64_STANDARD.encode(uuid)));
    } else {
        components.push(Component::error(4100));
        components.push(Component::error(4101));
        components.push(Component::error(4102));
    }

    components.push(Component::new(4145, "AQAAAA==".to_string()));
    components.push(Component::error(4160));
    components.push(Component::error(4161));

    // Common values sent with the request
    // "4128"
    // "4130"
    // "4112"
    // "4113"
    // "4098"
    // "4099"
    // "4100"
    // "4101"
    // "4102"
    // "4097"
    // "8195"
    // "8196"
    // "8197"
    // "4144"
    // "4145"
    // "4160"
    // "4161"

    components
}

#[cfg(target_os = "linux")]
fn load_smbios_fields(raw: Option<&[u8]>) -> io::Result<(Vec<u8>, Vec<u8>, [u8; 16])> {
    let smbios =
        raw.ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing raw SMBIOS data"))?;
    parse_smbios(smbios)
}

#[cfg(not(target_os = "linux"))]
fn load_smbios_fields(_raw: Option<&[u8]>) -> io::Result<(Vec<u8>, Vec<u8>, [u8; 16])> {
    let data = table_load_from_device()?;
    let system_info = data
        .first::<SMBiosSystemInformation>()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing SMBIOS Type 1"))?;

    let version = system_info
        .version()
        .to_utf8_lossy()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing SMBIOS version"))?
        .into_bytes();
    let serial = system_info
        .serial_number()
        .to_utf8_lossy()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing SMBIOS serial"))?
        .into_bytes();
    let uuid = match system_info.uuid() {
        Some(SystemUuidData::Uuid(uuid)) => uuid.raw,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing SMBIOS UUID",
            ));
        }
    };

    Ok((version, serial, uuid))
}

#[cfg(target_os = "linux")]
fn parse_smbios(smbios: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, [u8; 16])> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "malformed SMBIOS Type 1 table");
    let length = *smbios.get(1).ok_or_else(invalid)? as usize;
    let version = *smbios.get(6).ok_or_else(invalid)? as usize;
    let serial = *smbios.get(7).ok_or_else(invalid)? as usize;
    let uuid: [u8; 16] = smbios
        .get(8..24)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(invalid)?;
    let strings: Vec<&[u8]> = std::iter::once(&[][..])
        .chain(
            smbios
                .get(length..)
                .ok_or_else(invalid)?
                .split(|&b| b == 0)
                .take_while(|s| !s.is_empty()),
        )
        .collect();
    let string = |index: usize| strings.get(index).map(|s| s.to_vec()).unwrap_or_default();
    Ok((string(version), string(serial), uuid))
}

#[cfg(any(target_os = "macos", target_os = "ios", target_family = "windows"))]
fn load_raw_smbios() -> io::Result<Vec<u8>> {
    raw_smbios_from_device()
}

#[cfg(target_os = "linux")]
const SMBIOS_SYSTEM_TABLE: &str = "/sys/firmware/dmi/entries/1-0/raw";

#[cfg(target_os = "linux")]
fn load_raw_smbios() -> io::Result<Vec<u8>> {
    load_raw_smbios_from(std::path::Path::new(SMBIOS_SYSTEM_TABLE))
}

// Only root may read the table on most systems. Irori never asks for a
// password to register a device (Xodus runs `pkexec`): without the table the
// fields are sent as unavailable, as on boards without firmware DMI data.
#[cfg(target_os = "linux")]
fn load_raw_smbios_from(table: &std::path::Path) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    use std::io::Read;
    std::fs::File::open(table)?
        .take(64 * 1024)
        .read_to_end(&mut data)?;
    Ok(data)
}

#[cfg(all(test, target_os = "linux"))]
mod smbios_tests {
    use super::*;

    #[test]
    fn smbios_type1_strings_and_malformed_tables() {
        let mut table = vec![1, 24, 0, 0, 1, 2, 2, 3];
        table.extend(1..=16u8);
        table.extend(b"Maker\0Ver 1\0SER-9\0\0");
        let (version, serial, uuid) = parse_smbios(&table).unwrap();
        assert_eq!(version, b"Ver 1");
        assert_eq!(serial, b"SER-9");
        assert_eq!(uuid[0], 1);
        for short in [&table[..5], &table[..20]] {
            assert!(parse_smbios(short).is_err());
        }
        // A string index past the strings reads as empty, never panics.
        table[7] = 9;
        assert!(parse_smbios(&table).unwrap().1.is_empty());
    }

    #[test]
    fn smbios_missing_table_is_not_found() {
        let table = std::env::temp_dir().join(format!("xodus-no-smbios-{}", std::process::id()));
        let _ = std::fs::remove_file(&table);
        let error = load_raw_smbios_from(&table).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "ios",
    target_family = "windows"
)))]
fn load_raw_smbios() -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "raw SMBIOS loading is unsupported on this platform",
    ))
}

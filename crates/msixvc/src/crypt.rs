use crate::layout::PAGE_SIZE;
use crate::models::xvd::XvcRegionId;

use aes::cipher::{Array, BlockCipherDecrypt, BlockCipherEncrypt};
use aes::{Aes128Dec, Aes128Enc, Block};
use uuid::Uuid;

/// A [`TweakGenerator`] stores all common fields needed to generate every [`Tweak`]
/// for an XVC region.
#[derive(Clone, Copy, Debug)]
pub struct TweakGenerator {
    region_id: [u8; 4],
    vduid: [u8; 8],
}

impl TweakGenerator {
    pub fn new(region_id: XvcRegionId, vduid: Uuid) -> Self {
        Self {
            region_id: region_id.to_le_bytes(),
            vduid: vduid.to_bytes_le()[..8].try_into().unwrap(),
        }
    }

    pub fn with_data_unit(self, data_unit: u32) -> Tweak {
        let mut buf = [0u8; 16];

        buf[0..4].copy_from_slice(&data_unit.to_le_bytes());
        buf[4..8].copy_from_slice(&self.region_id);
        buf[8..16].copy_from_slice(&self.vduid);

        Tweak(buf)
    }
}

/// A [`Tweak`] is the per-page tweak input, derived from a [`TweakGenerator`] by adding
/// a unique `data_unit` via [`TweakGenerator::with_data_unit`].
#[derive(Clone, Copy, Debug)]
pub struct Tweak([u8; 16]);

impl Tweak {
    fn encrypt(self, tweak_cipher: &Aes128Enc) -> u128 {
        let mut block = Array(self.0);
        tweak_cipher.encrypt_block(&mut block);
        u128::from_le_bytes(block.0)
    }
}

/// Multiplies a polynomial by `x` in the Galois field `GF(2^128)` modulo
/// `x¹²⁸ + x⁷ + x² + x + 1`, the irreducible polynomial used by XTS-AES.
#[inline]
#[must_use = "unused arithmetic operation that must be used"]
const fn gf_mul_x(n: u128) -> u128 {
    // Shift all bits left by 1. If it overflows, XOR the result with the
    // field's irreducible polynomial (excluding the leading term).

    /// The irreducible polynomial used by XTS-AES: `x¹²⁸ + x⁷ + x² + x + 1`.
    /// The leading term `x¹²⁸` is implicit in the overflow bit and excluded here.
    const IRREDUCIBLE_POLYNOMIAL: u128 = 0x87;

    // If the high bit is set, then the mask is the irreducible polynomial
    // (excluding the leading term). If the high bit is not set, the mask is 0.
    let mask = (n >> 127).wrapping_neg() & IRREDUCIBLE_POLYNOMIAL;

    // Shift left and apply the mask.
    (n << 1) ^ mask
}

/// Transforms a page using XTS-AES (IEEE 1619-2007).
///
/// Each 16-byte block is transformed as `out = transform(in ⊕ T) ⊕ T`, where `T` is the
/// AES-encrypted tweak, advanced by one GF(2¹²⁸) multiplication per block.
///
/// The `transform` function receives independent blocks after their tweaks are applied, and should
/// perform either AES encryption or decryption.
#[inline]
fn transform_page_xts<F>(
    page: &mut [u8; PAGE_SIZE],
    tweak: Tweak,
    tweak_cipher: &Aes128Enc,
    transform: F,
) where
    F: Fn(&mut [Block]),
{
    // XTS requires the data length to be a multiple of the block size (16 bytes).
    const { assert!(PAGE_SIZE.is_multiple_of(16)) };

    // Let the AES backend interleave independent blocks instead of paying the
    // dispatch and AES-round dependency cost once per 16 bytes. A fixed stack
    // batch keeps memory bounded and leaves CPU feature selection to `aes`.
    const BLOCKS: usize = 8;
    const { assert!(PAGE_SIZE.is_multiple_of(16 * BLOCKS)) };
    let mut next_tweak = tweak.encrypt(tweak_cipher);
    for chunk in page.chunks_exact_mut(16 * BLOCKS) {
        let mut blocks = [Block::default(); BLOCKS];
        let mut tweaks = [0u128; BLOCKS];
        for (i, input) in chunk.chunks_exact(16).enumerate() {
            tweaks[i] = next_tweak;
            blocks[i] =
                Array((u128::from_le_bytes(input.try_into().unwrap()) ^ next_tweak).to_le_bytes());
            next_tweak = gf_mul_x(next_tweak);
        }
        transform(&mut blocks);
        for (i, output) in chunk.chunks_exact_mut(16).enumerate() {
            output.copy_from_slice(&(u128::from_le_bytes(blocks[i].0) ^ tweaks[i]).to_le_bytes());
        }
    }
}

/// Encrypts a page using XTS-AES (IEEE 1619-2007).
///
/// XTS-AES uses two keys: a tweak key to derive a per-page tweak, and a data key
/// to encrypt the data. Each 16-byte block is encrypted as `C = AES_enc(P ⊕ T) ⊕ T`,
/// where `T` is the AES-encrypted tweak, advanced by one GF(2¹²⁸) multiplication per block.
pub fn encrypt_page_xts(
    page: &mut [u8; PAGE_SIZE],
    tweak: Tweak,
    tweak_cipher: &Aes128Enc,
    data_cipher: &Aes128Enc,
) {
    transform_page_xts(page, tweak, tweak_cipher, |block| {
        data_cipher.encrypt_blocks(block);
    });
}

/// Decrypts a page using XTS-AES (IEEE 1619-2007).
///
/// XTS-AES uses two keys: a tweak key to derive a per-page tweak, and a data key
/// to decrypt the data. Each 16-byte block is decrypted as `P = AES_dec(C ⊕ T) ⊕ T`,
/// where `T` is the AES-encrypted tweak, advanced by one GF(2¹²⁸) multiplication per block.
pub fn decrypt_page_xts(
    page: &mut [u8; PAGE_SIZE],
    tweak: Tweak,
    tweak_cipher: &Aes128Enc,
    data_cipher: &Aes128Dec,
) {
    transform_page_xts(page, tweak, tweak_cipher, |block| {
        data_cipher.decrypt_blocks(block);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use aes::cipher::KeyInit;

    #[test]
    fn test_gf_mul_x() {
        // 0 * x = 0
        assert_eq!(gf_mul_x(0), 0);

        // 1 * x = x
        assert_eq!(gf_mul_x(1), 2);

        // High bit not set, so just a left shift.
        assert_eq!(gf_mul_x(0b101), 0b1010);

        // High bit set, so result must be reduced by XORing 0x87.
        assert_eq!(gf_mul_x(1u128 << 127), 0x87);

        // All bits are set: the shift overflows and the result is reduced.
        assert_eq!(gf_mul_x(u128::MAX), (u128::MAX << 1) ^ 0x87);
    }

    const TEST_PAGE: [u8; PAGE_SIZE] = [146; _];
    const TEST_TWEAK_CIPHER: &Block = &Array([178; _]);
    const TEST_DATA_CIPHER: &Block = &Array([22; _]);
    const TEST_REGION_ID: XvcRegionId = XvcRegionId::Other(1);
    const TEST_VDUID: Uuid = Uuid::from_bytes_le([222; _]);
    const TEST_DATA_UNIT: u32 = 67;

    #[test]
    fn test_transform_page_identity() {
        // Transforming a page with an identity function must return the same
        // page unchanged.

        let original_page = TEST_PAGE;
        let mut page = original_page;

        let tweak = TweakGenerator::new(TEST_REGION_ID, TEST_VDUID).with_data_unit(TEST_DATA_UNIT);
        let tweak_cipher = Aes128Enc::new(TEST_TWEAK_CIPHER);

        transform_page_xts(&mut page, tweak, &tweak_cipher, |_| ());

        assert_eq!(original_page, page);
    }

    #[test]
    fn test_encryption_decryption_round_trip() {
        // Encrypting and decrypting a page must return it unchanged.

        let original_page = TEST_PAGE;
        let mut page = original_page;

        let tweak = TweakGenerator::new(TEST_REGION_ID, TEST_VDUID).with_data_unit(TEST_DATA_UNIT);
        let tweak_cipher = Aes128Enc::new(TEST_TWEAK_CIPHER);
        let data_cipher_enc = Aes128Enc::new(TEST_DATA_CIPHER);
        let data_cipher_dec = Aes128Dec::new(TEST_DATA_CIPHER);

        encrypt_page_xts(&mut page, tweak, &tweak_cipher, &data_cipher_enc);

        assert_ne!(original_page, page);

        decrypt_page_xts(&mut page, tweak, &tweak_cipher, &data_cipher_dec);

        assert_eq!(original_page, page);
    }

    #[test]
    fn test_encryption_tweak() {
        // Encrypting with a different tweak must return a different ciphertext.

        let mut page1 = TEST_PAGE;
        let mut page2 = page1;

        let tweak_generator = TweakGenerator::new(TEST_REGION_ID, TEST_VDUID);
        let tweak1 = tweak_generator.with_data_unit(TEST_DATA_UNIT);
        let tweak2 = tweak_generator.with_data_unit(TEST_DATA_UNIT.wrapping_add(1));

        let tweak_cipher = Aes128Enc::new(TEST_TWEAK_CIPHER);
        let data_cipher_enc = Aes128Enc::new(TEST_DATA_CIPHER);

        encrypt_page_xts(&mut page1, tweak1, &tweak_cipher, &data_cipher_enc);
        encrypt_page_xts(&mut page2, tweak2, &tweak_cipher, &data_cipher_enc);

        assert_ne!(page1, page2);
    }
    #[test]
    #[ignore = "local CPU benchmark, not Internet bandwidth"]
    fn xts_page_decryption_benchmark() {
        use std::{hint::black_box, time::Instant};
        let tweak = TweakGenerator::new(TEST_REGION_ID, TEST_VDUID).with_data_unit(TEST_DATA_UNIT);
        let tweak_cipher = Aes128Enc::new(TEST_TWEAK_CIPHER);
        let cipher = Aes128Dec::new(TEST_DATA_CIPHER);
        let mut page = TEST_PAGE;
        for sample in 0..3 {
            let start = Instant::now();
            for _ in 0..32768 {
                decrypt_page_xts(
                    black_box(&mut page),
                    black_box(tweak),
                    black_box(&tweak_cipher),
                    black_box(&cipher),
                );
            }
            let seconds = start.elapsed().as_secs_f64();
            black_box(page);
            eprintln!(
                "XTS_CPU_BENCH sample={sample} bytes={} seconds={seconds:.6} MiBps={:.2}",
                32768 * PAGE_SIZE,
                128.0 / seconds
            );
        }
    }

    #[test]
    fn batched_xts_matches_scalar_reference_for_keys_units_and_both_directions() {
        fn scalar(
            page: &mut [u8; PAGE_SIZE],
            tweak: Tweak,
            key: &Aes128Enc,
            transform: impl Fn(&mut Block),
        ) {
            let mut value = tweak.encrypt(key);
            for bytes in page.chunks_exact_mut(16) {
                let mut block =
                    Array((u128::from_le_bytes(bytes.try_into().unwrap()) ^ value).to_le_bytes());
                transform(&mut block);
                bytes.copy_from_slice(&(u128::from_le_bytes(block.0) ^ value).to_le_bytes());
                value = gf_mul_x(value);
            }
        }
        for seed in 0..64u8 {
            let tweak_key =
                Aes128Enc::new(&Array(std::array::from_fn(|i| seed.wrapping_add(i as u8))));
            let data_key = Array(std::array::from_fn(|i| {
                seed.wrapping_mul(7).wrapping_add(i as u8)
            }));
            let enc = Aes128Enc::new(&data_key);
            let dec = Aes128Dec::new(&data_key);
            let tweak = TweakGenerator::new(
                XvcRegionId::Other(seed as u32),
                Uuid::from_bytes([seed; 16]),
            )
            .with_data_unit(u32::MAX.wrapping_sub(seed as u32));
            let plain = std::array::from_fn(|i| (i as u8).wrapping_mul(19).wrapping_add(seed));
            let mut expected = plain;
            scalar(&mut expected, tweak, &tweak_key, |block| {
                enc.encrypt_block(block)
            });
            let mut actual = plain;
            encrypt_page_xts(&mut actual, tweak, &tweak_key, &enc);
            assert_eq!(actual, expected);
            scalar(&mut expected, tweak, &tweak_key, |block| {
                dec.decrypt_block(block)
            });
            decrypt_page_xts(&mut actual, tweak, &tweak_key, &dec);
            assert_eq!(actual, expected);
            assert_eq!(actual, plain);
        }
    }

    #[test]
    fn xts_ciphertext_matches_independent_openssl_vector() {
        use sha2_11::{Digest, Sha256};
        // AES-128-XTS EVP, data key [22;16], tweak key [178;16], plaintext [146;4096].
        // IV is LE data unit67 + region1 + the first8 VDUUID bytes [222;8].
        let mut page = TEST_PAGE;
        let tweak = TweakGenerator::new(TEST_REGION_ID, TEST_VDUID).with_data_unit(TEST_DATA_UNIT);
        let tweak_cipher = Aes128Enc::new(TEST_TWEAK_CIPHER);
        encrypt_page_xts(
            &mut page,
            tweak,
            &tweak_cipher,
            &Aes128Enc::new(TEST_DATA_CIPHER),
        );
        assert_eq!(
            Sha256::digest(page)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "48dd83893608258ed7488c31c38dd6f6a94e7207d9b3c96eef7acde342b18f92"
        );
        decrypt_page_xts(
            &mut page,
            tweak,
            &tweak_cipher,
            &Aes128Dec::new(TEST_DATA_CIPHER),
        );
        assert_eq!(page, TEST_PAGE);
    }
}

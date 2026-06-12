//! Runtime EDID generation (port of scripts/gen-edid.py).
//! Lets the virtual display match any tablet resolution without shipping
//! pre-baked EDID binaries.

use anyhow::{Context, Result};
use std::path::PathBuf;

fn encode_manufacturer_id(s: &[u8; 3]) -> (u8, u8) {
    let c1 = (s[0] - b'A' + 1) as u16;
    let c2 = (s[1] - b'A' + 1) as u16;
    let c3 = (s[2] - b'A' + 1) as u16;
    let byte8 = ((c1 & 0x1F) << 2) | ((c2 >> 3) & 0x03);
    let byte9 = ((c2 & 0x07) << 5) | (c3 & 0x1F);
    (byte8 as u8, byte9 as u8)
}

/// Build a 128-byte EDID with a single detailed timing descriptor.
pub fn make_edid(width: u32, height: u32, refresh: u32) -> Vec<u8> {
    let mut edid = vec![0u8; 128];

    // Header
    edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);

    // Manufacturer ID (USC = UScreen)
    let (b8, b9) = encode_manufacturer_id(b"USC");
    edid[8] = b8;
    edid[9] = b9;

    // Product code / serial
    edid[10] = 0x01;
    edid[12..16].copy_from_slice(&1u32.to_le_bytes());

    edid[16] = 0; // week
    edid[17] = 34; // year (2024-1990)
    edid[18] = 1; // version
    edid[19] = 4; // revision

    edid[20] = 0xA5; // Digital, 8 bpc, DVI
    edid[21] = 31; // max image size cm
    edid[22] = 19;
    edid[23] = 0x78; // gamma 2.2
    edid[24] = 0xEE; // RGB, DPMS

    // Chromaticity (sRGB)
    edid[25..34].copy_from_slice(&[0xEF, 0x2C, 0xA2, 0xEE, 0xEF, 0x2C, 0xA2, 0x44, 0x42]);

    // No established timings; standard timings unused
    for i in 0..8 {
        edid[38 + i * 2] = 0x01;
        edid[39 + i * 2] = 0x01;
    }

    // CVT-RB style blanking
    let h_active = width;
    let v_active = height;
    let (h_front, h_sync, h_back) = (48u32, 32u32, 80u32);
    let h_blank = h_front + h_sync + h_back;
    let (v_front, v_sync, v_back) = (3u32, 10u32, 25u32);
    let v_blank = v_front + v_sync + v_back;

    let h_total = h_active + h_blank;
    let v_total = v_active + v_blank;
    let pixel_clock_10khz = ((h_total as u64 * v_total as u64 * refresh as u64 + 5000) / 10000)
        .min(u16::MAX as u64) as u16;

    let h_image = 310u32; // mm
    let v_image = 194u32;

    // === DTD 1 (bytes 54-71) ===
    let i = 54;
    edid[i..i + 2].copy_from_slice(&pixel_clock_10khz.to_le_bytes());
    edid[i + 2] = (h_active & 0xFF) as u8;
    edid[i + 3] = (h_blank & 0xFF) as u8;
    edid[i + 4] = ((((h_active >> 8) & 0x0F) << 4) | ((h_blank >> 8) & 0x0F)) as u8;
    edid[i + 5] = (v_active & 0xFF) as u8;
    edid[i + 6] = (v_blank & 0xFF) as u8;
    edid[i + 7] = ((((v_active >> 8) & 0x0F) << 4) | ((v_blank >> 8) & 0x0F)) as u8;
    edid[i + 8] = (h_front & 0xFF) as u8;
    edid[i + 9] = (h_sync & 0xFF) as u8;
    edid[i + 10] = (((v_front & 0x0F) << 4) | (v_sync & 0x0F)) as u8;
    edid[i + 11] = ((((h_front >> 8) & 0x03) << 6)
        | (((h_sync >> 8) & 0x03) << 4)
        | (((v_front >> 4) & 0x03) << 2)
        | ((v_sync >> 4) & 0x03)) as u8;
    edid[i + 12] = (h_image & 0xFF) as u8;
    edid[i + 13] = ((((h_image >> 8) & 0x0F) << 4) | ((v_image >> 8) & 0x0F)) as u8;
    edid[i + 14] = (v_image & 0xFF) as u8;
    edid[i + 17] = 0x1E; // non-interlaced, digital separate sync, +h +v

    // === Monitor name descriptor (bytes 90-107) ===
    let i = 90;
    edid[i + 3] = 0xFC;
    let name = b"UScreen\n     ";
    edid[i + 5..i + 5 + 13].copy_from_slice(&name[..13]);

    // === Range limits descriptor (bytes 108-125) ===
    let i = 108;
    edid[i + 3] = 0xFD;
    edid[i + 5] = 23; // min V rate Hz
    edid[i + 6] = 145; // max V rate Hz
    edid[i + 7] = 30; // min H rate kHz
    edid[i + 8] = 255; // max H rate kHz
    let max_pclk_mhz = (pixel_clock_10khz as u32 * 10_000 + 999_999) / 1_000_000;
    edid[i + 9] = ((max_pclk_mhz + 9) / 10).min(255) as u8;
    edid[i + 10] = 0x01; // GTF
    edid[i + 11..i + 18].copy_from_slice(b"\x0A      ");

    // Checksum
    let sum: u32 = edid[..127].iter().map(|&b| b as u32).sum();
    edid[127] = ((256 - (sum % 256)) % 256) as u8;

    edid
}

/// Write (or reuse) a generated EDID for this resolution and return its path.
pub fn ensure_edid(width: u32, height: u32, refresh: u32) -> Result<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir = PathBuf::from(home).join(".local/share/uscreen/edid");
    std::fs::create_dir_all(&dir).context("create EDID dir")?;
    let path = dir.join(format!("auto-{}x{}@{}.bin", width, height, refresh));
    if !path.exists() {
        std::fs::write(&path, make_edid(width, height, refresh)).context("write EDID")?;
        tracing::info!("Generated EDID for {}x{}@{} at {:?}", width, height, refresh, path);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_checksum_is_valid() {
        for (w, h) in [(2960u32, 1848u32), (2560, 1600), (1920, 1200), (2000, 1200)] {
            let edid = make_edid(w, h, 60);
            assert_eq!(edid.len(), 128);
            let sum: u32 = edid.iter().map(|&b| b as u32).sum();
            assert_eq!(sum % 256, 0, "checksum for {}x{}", w, h);
            // Decode DTD active size back
            let h_act = edid[56] as u32 | (((edid[58] >> 4) as u32) << 8);
            let v_act = edid[59] as u32 | (((edid[61] >> 4) as u32) << 8);
            assert_eq!((h_act, v_act), (w, h));
        }
    }
}

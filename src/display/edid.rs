use std::fmt;

use crate::error::{Error, Result};

/// Represents parsed EDID data for a display
#[derive(Debug, Clone)]
pub struct Edid {
    /// Manufacturer ID
    pub manufacturer: String,
    
    /// Product ID
    pub product_id: u16,
    
    /// Serial number
    pub serial_number: Option<u32>,
    
    /// Manufacture date
    pub manufacture_week: u8,
    pub manufacture_year: u16,
    
    /// EDID version
    pub version_major: u8,
    pub version_minor: u8,
    
    /// Display name
    pub name: Option<String>,
    
    /// Display size in cm
    pub width_cm: Option<u8>,
    pub height_cm: Option<u8>,
}

impl Edid {
    /// Parse EDID data from raw bytes
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 128 {
            return Err(Error::EdidParsing("EDID data too short".to_string()));
        }
        
        // Check EDID header
        let header = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
        if data[0..8] != header {
            return Err(Error::EdidParsing("Invalid EDID header".to_string()));
        }
        
        // Extract manufacturer ID (3 characters from 3 5-bit values)
        let manufacturer_id = ((data[8] as u16) << 8) | (data[9] as u16);
        let char1 = (((manufacturer_id >> 10) & 0x1F) + 64) as u8 as char;
        let char2 = (((manufacturer_id >> 5) & 0x1F) + 64) as u8 as char;
        let char3 = ((manufacturer_id & 0x1F) + 64) as u8 as char;
        let manufacturer = format!("{}{}{}", char1, char2, char3);
        
        // Extract product ID
        let product_id = ((data[11] as u16) << 8) | (data[10] as u16);
        
        // Extract serial number
        let serial_bytes = [data[12], data[13], data[14], data[15]];
        let serial_number = u32::from_le_bytes(serial_bytes);
        let serial_number = if serial_number == 0 { None } else { Some(serial_number) };
        
        // Extract manufacture date
        let manufacture_week = data[16];
        let manufacture_year = 1990 + data[17] as u16;
        
        // Extract EDID version
        let version_major = data[18];
        let version_minor = data[19];
        
        // Extract display size
        let width_cm = if data[21] != 0 { Some(data[21]) } else { None };
        let height_cm = if data[22] != 0 { Some(data[22]) } else { None };
        
        // Try to extract display name from descriptor blocks
        let mut name = None;
        
        // Descriptor blocks start at offset 54 and are 18 bytes each
        for i in 0..4 {
            let offset = 54 + i * 18;
            
            // Check if this is a display name descriptor
            if data[offset] == 0 && data[offset + 1] == 0 && data[offset + 2] == 0 && data[offset + 3] == 0xFC {
                // Extract the name (13 bytes, potentially null-terminated)
                let mut display_name = String::new();
                for j in 0..13 {
                    let byte = data[offset + 5 + j];
                    if byte == 0x0A || byte == 0x00 {
                        break;
                    }
                    display_name.push(byte as char);
                }
                name = Some(display_name);
                break;
            }
        }
        
        Ok(Edid {
            manufacturer,
            product_id,
            serial_number,
            manufacture_week,
            manufacture_year,
            version_major,
            version_minor,
            name,
            width_cm,
            height_cm,
        })
    }
    
    /// Generate a unique hash for this EDID
    ///
    /// This is 64-bit FNV-1a over a fixed byte layout, so the value saved in config
    /// files stays the same across Rust versions and platforms.
    pub fn hash(&self) -> String {
        const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut bytes = self.manufacturer.as_bytes().to_vec();
        bytes.extend_from_slice(&self.product_id.to_le_bytes());

        // Include serial number if available
        if let Some(serial) = self.serial_number {
            bytes.extend_from_slice(&serial.to_le_bytes());
        }

        let hash = bytes.iter()
            .fold(FNV_OFFSET_BASIS, |hash, &byte| (hash ^ byte as u64).wrapping_mul(FNV_PRIME));
        format!("{:016x}", hash)
    }
}

impl fmt::Display for Edid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} ({})",
            self.manufacturer,
            self.name.as_deref().unwrap_or("Unknown"),
            self.hash()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal EDID block: manufacturer "DEL", product 0x4321 and the given serial
    fn edid_block(serial: u32) -> Vec<u8> {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x10, 0xAC]);
        data[10..12].copy_from_slice(&[0x21, 0x43]);
        data[12..16].copy_from_slice(&serial.to_le_bytes());
        data
    }

    #[test]
    fn parses_identity_fields() {
        let edid = Edid::parse(&edid_block(0x12345678)).unwrap();
        assert_eq!(edid.manufacturer, "DEL");
        assert_eq!(edid.product_id, 0x4321);
        assert_eq!(edid.serial_number, Some(0x12345678));
    }

    // Hashes are saved in config files, so these values must never change
    #[test]
    fn hash_is_stable() {
        assert_eq!(Edid::parse(&edid_block(0x12345678)).unwrap().hash(), "4491eb5167355ef6");
        assert_eq!(Edid::parse(&edid_block(0)).unwrap().hash(), "fc287ad0f9821bb6");
    }

    #[test]
    fn rejects_short_data() {
        assert!(Edid::parse(&[0u8; 64]).is_err());
    }
}

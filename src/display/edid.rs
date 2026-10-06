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

    /// Serial number as stored in a text descriptor
    pub serial_string: Option<String>,
    
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
        
        // Descriptor blocks start at offset 54 and are 18 bytes each. Display descriptors
        // start with three zero bytes followed by a tag byte.
        let mut name = None;
        let mut serial_string = None;
        for i in 0..4 {
            let offset = 54 + i * 18;
            if data[offset..offset + 3] != [0, 0, 0] {
                continue;
            }

            match data[offset + 3] {
                0xFC if name.is_none() => name = Self::descriptor_text(&data[offset + 5..offset + 18]),
                0xFF if serial_string.is_none() => serial_string = Self::descriptor_text(&data[offset + 5..offset + 18]),
                _ => {}
            }
        }

        Ok(Edid {
            manufacturer,
            product_id,
            serial_number,
            serial_string,
            manufacture_week,
            manufacture_year,
            version_major,
            version_minor,
            name,
            width_cm,
            height_cm,
        })
    }

    /// Read the text of a display descriptor: up to 13 bytes, ended by a line feed and
    /// padded with spaces. Returns `None` for empty text.
    fn descriptor_text(bytes: &[u8]) -> Option<String> {
        let text: String = bytes.iter()
            .take_while(|&&byte| byte != 0x0A && byte != 0x00)
            .map(|&byte| byte as char)
            .collect();
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// The monitor's model name as it reports it, e.g. "Contoso C27". Monitors that
    /// report no name are described by manufacturer code and product ID instead.
    pub fn model_name(&self) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => format!("{} {:04X}", self.manufacturer, self.product_id),
        }
    }

    /// The serial number, preferring the text form many monitors store in a descriptor
    pub fn serial(&self) -> Option<String> {
        self.serial_string.clone().or_else(|| self.serial_number.map(|serial| serial.to_string()))
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

        // Many monitors leave the numeric serial at 0 and only store it as text, so
        // include that too. Otherwise two monitors of the same model would be identical.
        if let Some(serial) = &self.serial_string {
            bytes.extend_from_slice(serial.as_bytes());
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

    /// A minimal EDID block: manufacturer "ACM", product 0x4321 and the given serial
    fn edid_block(serial: u32) -> Vec<u8> {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x04, 0x6D]);
        data[10..12].copy_from_slice(&[0x21, 0x43]);
        data[12..16].copy_from_slice(&serial.to_le_bytes());
        data
    }

    /// Write a text display descriptor (tag 0xFC name, 0xFF serial) into descriptor slot `slot`
    fn with_descriptor(mut data: Vec<u8>, slot: usize, tag: u8, text: &str) -> Vec<u8> {
        let offset = 54 + slot * 18;
        data[offset..offset + 5].copy_from_slice(&[0, 0, 0, tag, 0]);
        let mut field = [b' '; 13];
        field[..text.len()].copy_from_slice(text.as_bytes());
        if text.len() < 13 {
            field[text.len()] = 0x0A;
        }
        data[offset + 5..offset + 18].copy_from_slice(&field);
        data
    }

    #[test]
    fn parses_identity_fields() {
        let edid = Edid::parse(&edid_block(0x12345678)).unwrap();
        assert_eq!(edid.manufacturer, "ACM");
        assert_eq!(edid.product_id, 0x4321);
        assert_eq!(edid.serial_number, Some(0x12345678));
    }

    #[test]
    fn parses_name_and_serial_descriptors() {
        let data = with_descriptor(with_descriptor(edid_block(0), 1, 0xFC, "Contoso C27"), 2, 0xFF, "ABC123");
        let edid = Edid::parse(&data).unwrap();
        assert_eq!(edid.name.as_deref(), Some("Contoso C27"));
        assert_eq!(edid.serial_string.as_deref(), Some("ABC123"));
        assert_eq!(edid.model_name(), "Contoso C27");
        assert_eq!(edid.serial().as_deref(), Some("ABC123"));
    }

    #[test]
    fn model_name_falls_back_to_manufacturer_and_product() {
        assert_eq!(Edid::parse(&edid_block(0)).unwrap().model_name(), "ACM 4321");
    }

    // Hashes are saved in config files, so these values must never change
    #[test]
    fn hash_is_stable() {
        assert_eq!(Edid::parse(&edid_block(0x12345678)).unwrap().hash(), "febf4ddcd53c05c8");
        assert_eq!(Edid::parse(&edid_block(0)).unwrap().hash(), "49a79d4ff6c79be0");
        // The model name doesn't identify a unit, so it must not change the hash
        let named = with_descriptor(edid_block(0), 1, 0xFC, "Contoso C27");
        assert_eq!(Edid::parse(&named).unwrap().hash(), "49a79d4ff6c79be0");
    }

    #[test]
    fn text_serial_tells_identical_models_apart() {
        let with_serial = with_descriptor(edid_block(0), 2, 0xFF, "ABC123");
        assert_eq!(Edid::parse(&with_serial).unwrap().hash(), "90c3d4627e9eda30");
    }

    #[test]
    fn rejects_short_data() {
        assert!(Edid::parse(&[0u8; 64]).is_err());
    }
}

//! The text inside a Dahua / Intelbras / Imou device QR code:
//! `{SN:9F0XXXXXXX,DT:iM5-SC,SC:abcdefgh}`

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceQr {
    pub serial: String,
    pub model: Option<String>,
    /// Safety code printed on the device; its factory password unless changed.
    pub safety_code: Option<String>,
}

pub fn parse(text: &str) -> Option<DeviceQr> {
    let inner = text.trim().trim_start_matches('{').trim_end_matches('}');
    let mut qr = DeviceQr { serial: String::new(), model: None, safety_code: None };
    for field in inner.split(',') {
        let Some((k, v)) = field.split_once(':') else { continue };
        let v = v.trim().to_string();
        match k.trim().to_ascii_uppercase().as_str() {
            "SN" => qr.serial = v,
            "DT" => qr.model = Some(v),
            "SC" => qr.safety_code = Some(v),
            _ => {}
        }
    }
    let ok = !qr.serial.is_empty() && qr.serial.chars().all(|c| c.is_ascii_alphanumeric());
    ok.then_some(qr)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses() {
        let q = super::parse("{SN:ABC123DEF,DT:iM5-SC,SC:xyz}").unwrap();
        assert_eq!(q.serial, "ABC123DEF");
        assert_eq!(q.model.as_deref(), Some("iM5-SC"));
        assert_eq!(q.safety_code.as_deref(), Some("xyz"));
        assert!(super::parse("hello").is_none());
    }
}

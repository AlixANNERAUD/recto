//! The Apple configuration profile that adds recto's printer to an iPhone, an iPad or a Mac
//! with a personal address, so the device never has to answer a password prompt (iOS often
//! shows none).
//!
//! The property list is written by hand: its structure is fixed and only a few strings vary,
//! which are escaped. The profile is not signed, so devices show it as unsigned: signing would
//! need a certificate devices trust, which a self-hosted recto has no way to provide.

use std::fmt::Write;

use uuid::Uuid;

/// What a profile holds.
pub struct AirPrintProfile<'a> {
    /// The app's name.
    pub brand: &'a str,
    /// The name the profile is listed under on the device.
    pub display_name: &'a str,
    /// What the device shows when asked to install it.
    pub description: &'a str,
    /// The printer's `printer-uuid`, which makes the identifiers stable.
    pub printer_uuid: Uuid,
    pub device_password_id: i64,
    pub host: &'a str,
    pub port: u16,
    pub tls: bool,
    /// The personal address's path: `/device/<secret>/ipp/print`.
    pub resource_path: &'a str,
}

impl AirPrintProfile<'_> {
    /// The profile as an unsigned property list. Its identifiers depend on the printer and the
    /// device password only, so downloading it again replaces the installed one instead of
    /// adding a second printer.
    pub fn to_xml(&self) -> String {
        let identifier = format!("recto.{}.{}", self.printer_uuid, self.device_password_id);
        let payload_identifier = format!("{identifier}.airprint");
        let uuid_of = |name: &str| {
            Uuid::new_v5(&self.printer_uuid, name.as_bytes())
                .to_string()
                .to_uppercase()
        };
        let boolean = |value: bool| if value { "<true/>" } else { "<false/>" };
        let mut xml = String::from(concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\">\n",
            "<dict>\n",
            "  <key>PayloadContent</key>\n",
            "  <array>\n",
            "    <dict>\n",
            "      <key>AirPrint</key>\n",
            "      <array>\n",
            "        <dict>\n",
        ));
        let _ = write!(
            xml,
            concat!(
                "          <key>ForceTLS</key>\n",
                "          {force_tls}\n",
                "          <key>IPAddress</key>\n",
                "          <string>{host}</string>\n",
                "          <key>Port</key>\n",
                "          <integer>{port}</integer>\n",
                "          <key>ResourcePath</key>\n",
                "          <string>{path}</string>\n",
                "        </dict>\n",
                "      </array>\n",
                "      <key>PayloadDisplayName</key>\n",
                "      <string>{name}</string>\n",
                "      <key>PayloadIdentifier</key>\n",
                "      <string>{payload_identifier}</string>\n",
                "      <key>PayloadType</key>\n",
                "      <string>com.apple.airprint</string>\n",
                "      <key>PayloadUUID</key>\n",
                "      <string>{payload_uuid}</string>\n",
                "      <key>PayloadVersion</key>\n",
                "      <integer>1</integer>\n",
                "    </dict>\n",
                "  </array>\n",
                "  <key>PayloadDescription</key>\n",
                "  <string>{description}</string>\n",
                "  <key>PayloadDisplayName</key>\n",
                "  <string>{name}</string>\n",
                "  <key>PayloadIdentifier</key>\n",
                "  <string>{identifier}</string>\n",
                "  <key>PayloadOrganization</key>\n",
                "  <string>{brand}</string>\n",
                "  <key>PayloadRemovalDisallowed</key>\n",
                "  <false/>\n",
                "  <key>PayloadType</key>\n",
                "  <string>Configuration</string>\n",
                "  <key>PayloadUUID</key>\n",
                "  <string>{uuid}</string>\n",
                "  <key>PayloadVersion</key>\n",
                "  <integer>1</integer>\n",
                "</dict>\n",
                "</plist>\n",
            ),
            force_tls = boolean(self.tls),
            host = text(self.host),
            port = self.port,
            path = text(self.resource_path),
            name = text(self.display_name),
            payload_identifier = text(&payload_identifier),
            payload_uuid = uuid_of(&payload_identifier),
            description = text(self.description),
            identifier = text(&identifier),
            brand = text(self.brand),
            uuid = uuid_of(&identifier),
        );
        xml
    }

    /// The file name it downloads as: the brand, with what a file name or a
    /// `Content-Disposition` header cannot carry replaced by hyphens.
    pub fn file_name(&self) -> String {
        let mut name = String::new();
        for c in self.brand.chars() {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                name.push(c);
            } else if !name.ends_with('-') {
                name.push('-');
            }
        }
        let name = name.trim_matches(['-', '.']);
        let name = if name.is_empty() { "printer" } else { name };
        format!("{name}.mobileconfig")
    }
}

/// `value` as the text of an element: escaped, without the characters XML 1.0 cannot carry
/// even escaped. Device names refuse control characters already; the brand and the host come
/// from the operator.
fn text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '\t' | '\n' | '\r' => escaped.push(c),
            c if c.is_control() || matches!(c, '\u{FFFE}' | '\u{FFFF}') => {}
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile<'a>(brand: &'a str, display_name: &'a str) -> AirPrintProfile<'a> {
        AirPrintProfile {
            brand,
            display_name,
            description: "Adds the printer.",
            printer_uuid: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            device_password_id: 7,
            host: "recto.example.org",
            port: 443,
            tls: true,
            resource_path: "/device/abcdefghijklmnopqrstuvwxyz234567/ipp/print",
        }
    }

    #[test]
    fn the_profile_is_one_airprint_payload() {
        let xml = profile("recto", "recto printer (Phone)").to_xml();
        let payload_uuid = Uuid::new_v5(
            &Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            b"recto.01234567-89ab-cdef-0123-456789abcdef.7.airprint",
        );
        let profile_uuid = Uuid::new_v5(
            &Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            b"recto.01234567-89ab-cdef-0123-456789abcdef.7",
        );
        let expected = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>AirPrint</key>
      <array>
        <dict>
          <key>ForceTLS</key>
          <true/>
          <key>IPAddress</key>
          <string>recto.example.org</string>
          <key>Port</key>
          <integer>443</integer>
          <key>ResourcePath</key>
          <string>/device/abcdefghijklmnopqrstuvwxyz234567/ipp/print</string>
        </dict>
      </array>
      <key>PayloadDisplayName</key>
      <string>recto printer (Phone)</string>
      <key>PayloadIdentifier</key>
      <string>recto.01234567-89ab-cdef-0123-456789abcdef.7.airprint</string>
      <key>PayloadType</key>
      <string>com.apple.airprint</string>
      <key>PayloadUUID</key>
      <string>{payload}</string>
      <key>PayloadVersion</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>PayloadDescription</key>
  <string>Adds the printer.</string>
  <key>PayloadDisplayName</key>
  <string>recto printer (Phone)</string>
  <key>PayloadIdentifier</key>
  <string>recto.01234567-89ab-cdef-0123-456789abcdef.7</string>
  <key>PayloadOrganization</key>
  <string>recto</string>
  <key>PayloadRemovalDisallowed</key>
  <false/>
  <key>PayloadType</key>
  <string>Configuration</string>
  <key>PayloadUUID</key>
  <string>{profile}</string>
  <key>PayloadVersion</key>
  <integer>1</integer>
</dict>
</plist>
"#,
            payload = payload_uuid.to_string().to_uppercase(),
            profile = profile_uuid.to_string().to_uppercase(),
        );
        assert_eq!(xml, expected);
    }

    #[test]
    fn plain_http_does_not_force_tls() {
        let xml = AirPrintProfile {
            port: 80,
            tls: false,
            ..profile("recto", "recto printer (Phone)")
        }
        .to_xml();
        assert!(
            xml.contains("<key>ForceTLS</key>\n          <false/>"),
            "{xml}"
        );
        assert!(xml.contains("<integer>80</integer>"), "{xml}");
    }

    #[test]
    fn profile_strings_are_escaped() {
        let xml = profile("A&B <Print>", "A&B printer (<&> \"Phone\"\u{7})").to_xml();
        assert!(
            xml.contains("<string>A&amp;B printer (&lt;&amp;&gt; \"Phone\")</string>"),
            "{xml}"
        );
        assert!(
            xml.contains("<string>A&amp;B &lt;Print&gt;</string>"),
            "{xml}"
        );
        assert!(!xml.contains("<&>"), "{xml}");
        assert!(!xml.contains('\u{7}'), "{xml}");
    }

    #[test]
    fn the_file_name_is_the_brand_made_safe() {
        assert_eq!(profile("recto", "").file_name(), "recto.mobileconfig");
        assert_eq!(
            profile("Print \"room\"/2", "").file_name(),
            "Print-room-2.mobileconfig"
        );
        assert_eq!(
            profile("Imprimerie é", "").file_name(),
            "Imprimerie.mobileconfig"
        );
        assert_eq!(profile("ééé", "").file_name(), "printer.mobileconfig");
    }
}

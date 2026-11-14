//! A minimal PDF writer: one JPEG per page, no compression of our own, no fonts. It exists so
//! the scan result needs no PDF dependency; the structure is fixed, so offsets are easy to track.

use std::fmt::Write as _;

pub struct PdfPage {
    pub jpeg: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
    pub gray: bool,
    pub dpi: u32,
}

/// Points with at most two decimals and no trailing zeros, so 300 px at 150 dpi is `144`.
fn points(px: u32, dpi: u32) -> String {
    let v = (f64::from(px) / f64::from(dpi.max(1)) * 72.0 * 100.0).round() / 100.0;
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

pub fn write(pages: &[PdfPage]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    // Object numbers: 1 catalog, 2 page tree, then page/content/image triples from 3.
    let mut object = |out: &mut Vec<u8>, body: &[u8]| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };

    object(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: String = (0..pages.len()).fold(String::new(), |mut s, i| {
        let _ = write!(s, "{} 0 R ", 3 + i * 3);
        s
    });
    object(
        &mut out,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );

    for (i, p) in pages.iter().enumerate() {
        let (w, h) = (points(p.width_px, p.dpi), points(p.height_px, p.dpi));
        let first = 3 + i * 3;
        object(
            &mut out,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] \
                 /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>",
                first + 2,
                first + 1
            )
            .as_bytes(),
        );
        let content = format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q");
        object(
            &mut out,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )
            .as_bytes(),
        );
        let space = if p.gray { "DeviceGray" } else { "DeviceRGB" };
        let mut image = format!(
            "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /{space} \
             /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
            p.width_px,
            p.height_px,
            p.jpeg.len()
        )
        .into_bytes();
        image.extend_from_slice(&p.jpeg);
        image.extend_from_slice(b"\nendstream");
        object(&mut out, &image);
    }

    let xref = out.len();
    let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in &offsets {
        let _ = writeln!(table, "{o:010} 00000 n ");
    }
    let _ = write!(
        table,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len() + 1
    );
    out.extend_from_slice(table.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let pdf = write(&[
            PdfPage {
                jpeg: vec![0xFF, 0xD8, 0xFF, 0xD9],
                width_px: 300,
                height_px: 600,
                gray: false,
                dpi: 150,
            },
            PdfPage {
                jpeg: vec![1, 2, 3],
                width_px: 100,
                height_px: 100,
                gray: true,
                dpi: 72,
            },
        ]);
        let start = find(&pdf, b"startxref\n").unwrap() + "startxref\n".len();
        let end = start + pdf[start..].iter().position(|b| *b == b'\n').unwrap();
        let xref: usize = std::str::from_utf8(&pdf[start..end])
            .unwrap()
            .parse()
            .unwrap();
        assert!(pdf[xref..].starts_with(b"xref\n0 9\n"));
        let entries = &pdf[xref + "xref\n0 9\n".len()..];
        for n in 1..9usize {
            let line = std::str::from_utf8(&entries[n * 20..n * 20 + 20]).unwrap();
            let offset: usize = line[..10].parse().unwrap();
            assert!(
                pdf[offset..].starts_with(format!("{n} 0 obj\n").as_bytes()),
                "object {n}"
            );
        }
    }
}

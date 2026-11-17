// SPDX-License-Identifier: GPL-3.0-or-later
//
// Schweizer QR-Rechnung (Zahlteil mit Empfangsschein, 210 × 105 mm) als PDF,
// nach SIX Implementation Guidelines QR-Rechnung 2.3: «SPC» 0200, UTF-8,
// Fehlerkorrektur M, 46 × 46 mm mit Schweizerkreuz. Übernommen aus
// adhs-expert/src/qrrechnung.rs, hier mit SCOR-Referenz (ISO 11649) je
// Bestellung - so lässt sich eine Zahlung später automatisch zuordnen.
//
// Gewöhnliche IBAN: Referenztyp SCOR. Eine QR-IBAN (IID 30000-31999) bräuchte
// eine QR-Referenz und wird abgewiesen.

use anyhow::{bail, Context, Result};
use printpdf::{Color, Greyscale, IndirectFontRef, Line, LineDashPattern, Mm, PdfDocument, PdfLayerReference, Point};
use rusttype::{Font, Scale};

const BREITE: f64 = 210.0;
const HOEHE: f64 = 105.0;
const EMPFANG: f64 = 62.0;
const PT_JE_MM: f64 = 72.0 / 25.4;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Glaeubiger {
    pub iban: String,
    pub name: String,
    pub strasse: String,
    pub hausnr: String,
    pub plz: String,
    pub ort: String,
    #[serde(default = "land_ch")]
    pub land: String,
    /// UID/MWST-Nummer, geht als Swico S1 in den Code
    pub uid: Option<String>,
}

fn land_ch() -> String {
    "CH".into()
}

pub struct Rechnung<'a> {
    pub glaeubiger: &'a Glaeubiger,
    /// Betrag mit zwei Nachkommastellen, etwa «50.00»
    pub betrag: String,
    pub waehrung: String,
    /// SCOR-Referenz ohne Leerzeichen, etwa «RF18539007547034»
    pub referenz: String,
    pub mitteilung: String,
}

/// IBAN ohne Leerzeichen, gross, geprüft (CH/LI, 21 Stellen, mod 97, keine QR-IBAN).
pub fn iban_pruefen(iban: &str) -> Result<String> {
    let iban: String = iban.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    if !(iban.starts_with("CH") || iban.starts_with("LI")) || iban.len() != 21 {
        bail!("IBAN {iban}: nur CH/LI mit 21 Stellen");
    }
    if mod97(&format!("{}{}", &iban[4..], &iban[..4]))? != 1 {
        bail!("IBAN {iban}: Prüfziffer falsch");
    }
    let iid: u32 = iban[4..9].parse().unwrap_or(0);
    if (30000..=31999).contains(&iid) {
        bail!("{iban} ist eine QR-IBAN: dafür braucht es eine QR-Referenz statt SCOR");
    }
    Ok(iban)
}

/// Rest modulo 97 einer Zeichenkette aus Ziffern und Buchstaben (A=10 … Z=35).
fn mod97(s: &str) -> Result<u32> {
    let mut rest: u32 = 0;
    for c in s.chars() {
        let v = c.to_digit(36).with_context(|| format!("Zeichen {c:?}"))?;
        for d in v.to_string().chars() {
            rest = (rest * 10 + d.to_digit(10).unwrap_or(0)) % 97;
        }
    }
    Ok(rest)
}

/// SCOR-Referenz nach ISO 11649 zu `basis` (1-21 Ziffern/Buchstaben).
pub fn scor(basis: &str) -> Result<String> {
    let basis = basis.to_uppercase();
    if basis.is_empty() || basis.len() > 21 || !basis.chars().all(|c| c.is_ascii_alphanumeric()) {
        bail!("SCOR-Basis {basis:?}: 1-21 Ziffern oder Buchstaben");
    }
    let pruef = 98 - mod97(&format!("{basis}RF00"))?;
    Ok(format!("RF{pruef:02}{basis}"))
}

pub fn scor_gueltig(r: &str) -> bool {
    r.len() > 4 && r.starts_with("RF") && mod97(&format!("{}{}", &r[4..], &r[..4])).is_ok_and(|m| m == 1)
}

fn rechnungsinfo(g: &Glaeubiger) -> Option<String> {
    let uid: String = g.uid.as_ref()?.chars().filter(char::is_ascii_digit).collect();
    (uid.len() == 9).then(|| format!("//S1/30/{uid}"))
}

/// Die Daten im QR-Code, Feld für Feld nach SIX IG 2.3, Kapitel 4.
pub fn nutzdaten(r: &Rechnung) -> String {
    let g = r.glaeubiger;
    let mut z: Vec<String> = vec!["SPC".into(), "0200".into(), "1".into(), g.iban.clone()];
    z.extend(["S", &g.name, &g.strasse, &g.hausnr, &g.plz, &g.ort, &g.land].map(String::from));
    z.extend(std::iter::repeat_n(String::new(), 7)); // endgültiger Zahlungsempfänger
    z.push(r.betrag.clone());
    z.push(r.waehrung.clone());
    z.extend(std::iter::repeat_n(String::new(), 7)); // Zahlungspflichtiger
    z.push("SCOR".into());
    z.push(r.referenz.clone());
    z.push(r.mitteilung.clone());
    z.push("EPD".into());
    if let Some(i) = rechnungsinfo(g) {
        z.push(i);
    }
    z.join("\r\n")
}

fn gruppen(s: &str, n: usize) -> String {
    s.chars().collect::<Vec<_>>().chunks(n).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

struct Satz<'a> {
    ebene: &'a PdfLayerReference,
    normal: &'a IndirectFontRef,
    fett: &'a IndirectFontRef,
    mass: &'a Font<'static>,
}

impl Satz<'_> {
    fn text(&self, s: &str, pt: f64, x: f64, y: f64, fett: bool) {
        self.ebene.use_text(s, pt, Mm(x), Mm(HOEHE - y), if fett { self.fett } else { self.normal });
    }
    fn breite(&self, s: &str, pt: f64) -> f64 {
        let v = self.mass.v_metrics_unscaled();
        let geviert = (v.ascent - v.descent) / self.mass.units_per_em() as f32;
        let sc = Scale::uniform(pt as f32 * geviert);
        let w: f32 = s.chars().map(|c| self.mass.glyph(c).scaled(sc).h_metrics().advance_width).sum();
        w as f64 / PT_JE_MM
    }
    fn umbrechen(&self, s: &str, pt: f64, max: f64) -> Vec<String> {
        let mut zeilen = Vec::new();
        let mut z = String::new();
        for w in s.split_whitespace() {
            let probe = if z.is_empty() { w.to_string() } else { format!("{z} {w}") };
            if !z.is_empty() && self.breite(&probe, pt) > max {
                zeilen.push(std::mem::take(&mut z));
                z = w.to_string();
            } else {
                z = probe;
            }
        }
        if !z.is_empty() {
            zeilen.push(z);
        }
        zeilen
    }
    fn flaeche(&self, x: f64, y: f64, b: f64, h: f64, grau: f64) {
        self.ebene.set_fill_color(Color::Greyscale(Greyscale::new(grau, None)));
        let p = |x: f64, y: f64| (Point::new(Mm(x), Mm(HOEHE - y)), false);
        self.ebene.add_shape(Line {
            points: vec![p(x, y), p(x + b, y), p(x + b, y + h), p(x, y + h)],
            is_closed: true,
            has_fill: true,
            has_stroke: false,
            is_clipping_path: false,
        });
    }
    fn strich(&self, punkte: &[(f64, f64)]) {
        let p = |x: f64, y: f64| (Point::new(Mm(x), Mm(HOEHE - y)), false);
        self.ebene.add_shape(Line {
            points: punkte.iter().map(|&(x, y)| p(x, y)).collect(),
            is_closed: false,
            has_fill: false,
            has_stroke: true,
            is_clipping_path: false,
        });
    }
    fn eckfeld(&self, x: f64, y: f64, b: f64, h: f64) {
        let e = 3.0;
        self.ebene.set_outline_thickness(0.75);
        self.strich(&[(x, y + e), (x, y), (x + e, y)]);
        self.strich(&[(x + b - e, y), (x + b, y), (x + b, y + e)]);
        self.strich(&[(x, y + h - e), (x, y + h), (x + e, y + h)]);
        self.strich(&[(x + b - e, y + h), (x + b, y + h), (x + b, y + h - e)]);
    }
    /// Überschrift und umbrochene Werte; gibt die nächste freie Zeile zurück.
    #[allow(clippy::too_many_arguments)]
    fn block(&self, titel: &str, werte: &[String], x: f64, mut y: f64, kopf_pt: f64, pt: f64, max: f64) -> f64 {
        let zeile = pt * 0.42;
        self.text(titel, kopf_pt, x, y, true);
        y += zeile;
        for w in werte {
            for t in self.umbrechen(w, pt, max) {
                self.text(&t, pt, x, y, false);
                y += zeile;
            }
        }
        y + 2.5
    }
}

fn qr_zeichnen(s: &Satz, daten: &str, x0: f64, y0: f64) -> Result<()> {
    let code = qrcode::QrCode::with_error_correction_level(daten.as_bytes(), qrcode::EcLevel::M)
        .context("QR-Code: Daten zu lang")?;
    let n = code.width();
    let m = 46.0 / n as f64;
    for y in 0..n {
        let mut x = 0;
        while x < n {
            if code[(x, y)] == qrcode::Color::Dark {
                let start = x;
                while x < n && code[(x, y)] == qrcode::Color::Dark {
                    x += 1;
                }
                s.flaeche(x0 + start as f64 * m, y0 + y as f64 * m, (x - start) as f64 * m + 0.01, m + 0.01, 0.0);
            } else {
                x += 1;
            }
        }
    }
    let (cx, cy) = (x0 + 23.0, y0 + 23.0);
    s.flaeche(cx - 3.5, cy - 3.5, 7.0, 7.0, 1.0);
    s.flaeche(cx - 3.0, cy - 3.0, 6.0, 6.0, 0.0);
    let (lang, dick) = (3.9, 1.17);
    s.flaeche(cx - dick / 2.0, cy - lang / 2.0, dick, lang, 1.0);
    s.flaeche(cx - lang / 2.0, cy - dick / 2.0, lang, dick, 1.0);
    s.ebene.set_fill_color(Color::Greyscale(Greyscale::new(0.0, None)));
    Ok(())
}

pub struct Schriften {
    pub normal: Vec<u8>,
    pub fett: Vec<u8>,
}

impl Schriften {
    /// Arial aus dem System (SIX erlaubt Arial, Frutiger, Helvetica,
    /// Liberation Sans); Pfade aus der Konfiguration.
    pub fn laden(normal: &str, fett: &str) -> Result<Self> {
        Ok(Schriften {
            normal: std::fs::read(normal).with_context(|| format!("Schrift {normal}"))?,
            fett: std::fs::read(fett).with_context(|| format!("Schrift {fett}"))?,
        })
    }
}

/// Die QR-Rechnung als PDF-Bytes.
pub fn pdf(r: &Rechnung, schriften: &Schriften) -> Result<Vec<u8>> {
    let g = r.glaeubiger;
    if !scor_gueltig(&r.referenz) {
        bail!("Referenz {} ist keine gültige SCOR-Referenz", r.referenz);
    }
    let daten = nutzdaten(r);
    let mass = Font::from_bytes(schriften.fett.clone()).context("Schrift nicht lesbar")?;
    let (doc, seite, ebene) = PdfDocument::new("QR-Rechnung", Mm(BREITE), Mm(HOEHE), "Zahlteil");
    let normal = doc.add_external_font(std::io::Cursor::new(schriften.normal.clone())).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let fett = doc.add_external_font(std::io::Cursor::new(schriften.fett.clone())).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let ebene = doc.get_page(seite).get_layer(ebene);
    let s = Satz { ebene: &ebene, normal: &normal, fett: &fett, mass: &mass };

    let mut konto = vec![gruppen(&g.iban, 4), g.name.clone()];
    let strasse = format!("{} {}", g.strasse, g.hausnr).trim().to_string();
    if !strasse.is_empty() {
        konto.push(strasse);
    }
    konto.push(format!("{} {}", g.plz, g.ort));
    let referenz = vec![gruppen(&r.referenz, 4)];
    let mut zusatz = vec![r.mitteilung.clone()];
    if let Some(i) = rechnungsinfo(g) {
        zusatz.push(i);
    }

    ebene.set_outline_thickness(0.5);
    ebene.set_line_dash_pattern(LineDashPattern::new(0, Some(2), Some(2), None, None, None, None));
    s.strich(&[(EMPFANG, 0.0), (EMPFANG, HOEHE)]);
    ebene.set_line_dash_pattern(LineDashPattern::new(0, None, None, None, None, None, None));

    // Empfangsschein: Überschriften 6 pt, Werte 8 pt
    s.text("Empfangsschein", 11.0, 5.0, 9.0, true);
    let mut y = s.block("Konto / Zahlbar an", &konto, 5.0, 15.0, 6.0, 8.0, 52.0);
    y = s.block("Referenz", &referenz, 5.0, y, 6.0, 8.0, 52.0);
    s.text("Zahlbar durch (Name/Adresse)", 6.0, 5.0, y, true);
    s.eckfeld(5.0, y + 1.5, 52.0, 20.0);
    s.text("Währung", 6.0, 5.0, 70.0, true);
    s.text("Betrag", 6.0, 18.0, 70.0, true);
    s.text(&r.waehrung, 8.0, 5.0, 74.0, false);
    s.text(&r.betrag, 8.0, 18.0, 74.0, false);
    let w = s.breite("Annahmestelle", 6.0);
    s.text("Annahmestelle", 6.0, EMPFANG - 5.0 - w, 88.0, true);

    // Zahlteil: Überschriften 8 pt, Werte 10 pt
    let x = EMPFANG + 5.0;
    s.text("Zahlteil", 11.0, x, 9.0, true);
    qr_zeichnen(&s, &daten, x, 17.0)?;
    s.text("Währung", 8.0, x, 70.0, true);
    s.text("Betrag", 8.0, x + 15.0, 70.0, true);
    s.text(&r.waehrung, 10.0, x, 75.0, false);
    s.text(&r.betrag, 10.0, x + 15.0, 75.0, false);
    let xr = EMPFANG + 56.0;
    let platz = BREITE - xr - 5.0;
    let mut y = s.block("Konto / Zahlbar an", &konto, xr, 9.0, 8.0, 10.0, platz);
    y = s.block("Referenz", &referenz, xr, y, 8.0, 10.0, platz);
    y = s.block("Zusätzliche Informationen", &zusatz, xr, y, 8.0, 10.0, platz);
    s.text("Zahlbar durch (Name/Adresse)", 8.0, xr, y, true);
    s.eckfeld(xr, y + 1.5, 65.0, 20.0);

    let mut puffer = std::io::BufWriter::new(Vec::new());
    doc.save(&mut puffer).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    Ok(puffer.into_inner()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glaeubiger() -> Glaeubiger {
        Glaeubiger {
            iban: "CH9300762011623852957".into(),
            name: "Muster AG".into(),
            strasse: "Weg".into(),
            hausnr: "1".into(),
            plz: "8000".into(),
            ort: "Zürich".into(),
            land: "CH".into(),
            uid: Some("CHE-123.456.789 MWST".into()),
        }
    }

    #[test]
    fn iban() {
        assert_eq!(iban_pruefen("CH93 0076 2011 6238 5295 7").unwrap(), "CH9300762011623852957");
        assert!(iban_pruefen("CH9300762011623852958").is_err());
        assert!(iban_pruefen("CH4431999123000889012").is_err()); // QR-IBAN
    }

    #[test]
    fn scor_referenz() {
        // Beispiel aus ISO 11649
        assert_eq!(scor("539007547034").unwrap(), "RF18539007547034");
        assert!(scor_gueltig("RF18539007547034"));
        assert!(!scor_gueltig("RF19539007547034"));
    }

    #[test]
    fn nutzdaten_mit_scor() {
        let g = glaeubiger();
        let r = Rechnung {
            glaeubiger: &g,
            betrag: "50.00".into(),
            waehrung: "CHF".into(),
            referenz: "RF18539007547034".into(),
            mitteilung: "Test".into(),
        };
        let d = nutzdaten(&r);
        let z: Vec<&str> = d.split("\r\n").collect();
        assert_eq!(z[18], "50.00");
        assert_eq!(z[27], "SCOR");
        assert_eq!(z[28], "RF18539007547034");
        assert_eq!(z[30], "EPD");
        assert_eq!(z[31], "//S1/30/123456789");
    }
}

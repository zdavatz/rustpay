// SPDX-License-Identifier: GPL-3.0-or-later
//
// Kataloge: viele gleichartige Produkte aus einer JSON-Datei, statt jedes von
// Hand in kasse.toml (Wunsch des Benutzers, November 2026: «jedes Vortrags-PDF
// von ganglion.ch kostet ab sofort CHF 10.–»). Die Datei erzeugt ein Skript
// aus der Datenbank der Webseite (ganglion: kasse_katalog.php):
//
//   [{"nr": 123, "titel": "…", "beschreibung": "…",
//     "dateien": [{"name": "Titel.pdf", "pfad": "/…/datei.pdf"}]}, …]
//
// Das Produkt heisst `<praefix>-<nr>`, etwa `vortrag-123`. Die Datei wird
// neu gelesen, sobald sich ihr Änderungsdatum ändert - kein Neustart nötig.
// Einträge ohne vorhandene Datei werden übersprungen.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::konfig::{Datei, Produkt};

#[derive(Debug, Deserialize, Clone)]
pub struct KatalogKonfig {
    /// Produkt-ID = `<praefix>-<nr>`
    pub praefix: String,
    /// vor den Titel gesetzt, etwa «Vortrag: »
    #[serde(default)]
    pub vorsatz: String,
    /// gilt für Einträge ohne eigene Beschreibung
    #[serde(default)]
    pub beschreibung: String,
    pub preis: String,
    #[serde(default = "chf")]
    pub waehrung: String,
    #[serde(default = "dreissig")]
    pub gueltig_tage: i64,
    /// JSON-Datei mit den Einträgen
    pub datei: PathBuf,
}

fn chf() -> String {
    "CHF".into()
}

fn dreissig() -> i64 {
    30
}

#[derive(Debug, Deserialize, Clone)]
struct Eintrag {
    nr: u64,
    titel: String,
    #[serde(default)]
    beschreibung: String,
    dateien: Vec<Datei>,
}

pub struct Katalog {
    pub k: KatalogKonfig,
    stand: Mutex<(Option<SystemTime>, HashMap<u64, Eintrag>)>,
}

impl Katalog {
    pub fn neu(k: KatalogKonfig) -> Result<Self> {
        if k.praefix.is_empty() || !k.praefix.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) {
            bail!("Katalog-Präfix {:?}: nur a-z und 0-9", k.praefix);
        }
        let v: f64 = k.preis.parse().with_context(|| format!("Katalog {}: Preis", k.praefix))?;
        if !(0.01..=999_999_999.99).contains(&v) {
            bail!("Katalog {}: Preis ausserhalb 0.01 bis 999999999.99", k.praefix);
        }
        let kat = Katalog { k, stand: Mutex::new((None, HashMap::new())) };
        kat.auffrischen()?;
        Ok(kat)
    }

    /// Liest die JSON-Datei, wenn sie sich seit dem letzten Mal geändert hat.
    fn auffrischen(&self) -> Result<()> {
        let geaendert = std::fs::metadata(&self.k.datei)
            .and_then(|m| m.modified())
            .with_context(|| format!("Katalog {}", self.k.datei.display()))?;
        let mut stand = self.stand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?;
        if stand.0 == Some(geaendert) {
            return Ok(());
        }
        let text = std::fs::read_to_string(&self.k.datei)?;
        let eintraege: Vec<Eintrag> =
            serde_json::from_str(&text).with_context(|| format!("Katalog {}", self.k.datei.display()))?;
        let mut neu = HashMap::new();
        for mut e in eintraege {
            e.dateien.retain(|d| d.pfad.is_file());
            if !e.dateien.is_empty() {
                neu.insert(e.nr, e);
            }
        }
        *stand = (Some(geaendert), neu);
        Ok(())
    }

    pub fn anzahl(&self) -> usize {
        self.stand.lock().map(|s| s.1.len()).unwrap_or(0)
    }

    /// Produkt zu `<praefix>-<nr>`, falls die ID zu diesem Katalog gehört.
    pub fn produkt(&self, id: &str) -> Option<Produkt> {
        let nr: u64 = id.strip_prefix(&self.k.praefix)?.strip_prefix('-')?.parse().ok()?;
        if let Err(e) = self.auffrischen() {
            eprintln!("Katalog {}: {e:#}", self.k.praefix);
        }
        let stand = self.stand.lock().ok()?;
        let e = stand.1.get(&nr)?;
        Some(Produkt {
            id: id.to_string(),
            titel: format!("{}{}", self.k.vorsatz, e.titel),
            beschreibung: if e.beschreibung.is_empty() { self.k.beschreibung.clone() } else { e.beschreibung.clone() },
            preis: self.k.preis.clone(),
            waehrung: self.k.waehrung.clone(),
            gueltig_tage: self.k.gueltig_tage,
            dateien: e.dateien.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eintraege_und_ids() {
        let ordner = std::env::temp_dir().join(format!("kasse-kat-{}", crate::bestand::schluessel()));
        std::fs::create_dir_all(&ordner).unwrap();
        let pdf = ordner.join("v.pdf");
        std::fs::write(&pdf, b"%PDF").unwrap();
        let json = ordner.join("k.json");
        std::fs::write(
            &json,
            format!(
                r#"[{{"nr": 7, "titel": "Angst", "dateien": [{{"name": "Angst.pdf", "pfad": "{p}"}}]}},
                   {{"nr": 8, "titel": "Ohne Datei", "dateien": [{{"name": "x.pdf", "pfad": "/gibt/es/nicht.pdf"}}]}}]"#,
                p = pdf.display()
            ),
        )
        .unwrap();
        let k = Katalog::neu(KatalogKonfig {
            praefix: "vortrag".into(),
            vorsatz: "Vortrag: ".into(),
            beschreibung: "PDF des Vortrags".into(),
            preis: "10.00".into(),
            waehrung: "CHF".into(),
            gueltig_tage: 30,
            datei: json,
        })
        .unwrap();
        assert_eq!(k.anzahl(), 1, "Eintrag ohne Datei fällt weg");
        let p = k.produkt("vortrag-7").unwrap();
        assert_eq!(p.titel, "Vortrag: Angst");
        assert_eq!(p.preis, "10.00");
        assert_eq!(p.beschreibung, "PDF des Vortrags");
        assert!(k.produkt("vortrag-8").is_none());
        assert!(k.produkt("vortrag-x").is_none());
        assert!(k.produkt("artikel-7").is_none());
        assert!(k.produkt("vortrag7").is_none());
        std::fs::remove_dir_all(ordner).ok();
    }
}

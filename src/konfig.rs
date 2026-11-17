// SPDX-License-Identifier: GPL-3.0-or-later
//
// Konfiguration aus einer TOML-Datei (Vorlage: kasse.toml.beispiel). Die
// echte Datei trägt Kontodaten und das Admin-Passwort und gehört nicht ins
// Repository.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::rechnung::{self, Glaeubiger};

#[derive(Debug, Deserialize)]
pub struct Konfig {
    pub server: Server,
    pub mail: Mail,
    pub glaeubiger: Glaeubiger,
    #[serde(rename = "produkt", default)]
    pub produkte: Vec<Produkt>,
    /// viele gleichartige Produkte aus einer JSON-Datei (siehe katalog.rs)
    #[serde(rename = "katalog", default)]
    pub kataloge: Vec<crate::katalog::KatalogKonfig>,
}

#[derive(Debug, Deserialize)]
pub struct Server {
    /// etwa «127.0.0.1:3001» (Apache leitet /kasse/ dorthin weiter)
    pub adresse: String,
    /// öffentliche Adresse ohne Schrägstrich am Ende, etwa «https://ganglion.ch/kasse»
    pub basis_url: String,
    /// Ordner für die SQLite-Datenbank
    pub daten: PathBuf,
    pub admin_benutzer: String,
    pub admin_passwort: String,
    #[serde(default = "schrift_normal")]
    pub schrift_normal: String,
    #[serde(default = "schrift_fett")]
    pub schrift_fett: String,
}

fn schrift_normal() -> String {
    "/System/Library/Fonts/Supplemental/Arial.ttf".into()
}

fn schrift_fett() -> String {
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf".into()
}

#[derive(Debug, Deserialize)]
pub struct Mail {
    /// Absender, etwa «Muster GmbH <verkauf@example.ch>»
    pub absender: String,
    /// erhält jede Bestellung und jede Freigabe in Kopie (Bcc)
    pub kopie: Option<String>,
    /// Pfad zu sendmail (auf ganglion.ch msmtp → Google-Workspace-Relay)
    #[serde(default = "sendmail")]
    pub sendmail: String,
    /// Statt zu senden die Mails als .eml in diesen Ordner schreiben (Tests,
    /// lokaler Betrieb)
    pub ordner: Option<PathBuf>,
}

fn sendmail() -> String {
    "/usr/sbin/sendmail".into()
}

#[derive(Debug, Deserialize, Clone)]
pub struct Produkt {
    /// kurz, nur Kleinbuchstaben/Ziffern/Bindestrich: erscheint in der Adresse
    pub id: String,
    pub titel: String,
    #[serde(default)]
    pub beschreibung: String,
    /// «50.00»
    pub preis: String,
    #[serde(default = "chf")]
    pub waehrung: String,
    /// Tage, die der Download-Link nach der Freigabe gilt
    #[serde(default = "dreissig")]
    pub gueltig_tage: i64,
    #[serde(rename = "datei")]
    pub dateien: Vec<Datei>,
}

fn chf() -> String {
    "CHF".into()
}

fn dreissig() -> i64 {
    30
}

#[derive(Debug, Deserialize, Clone)]
pub struct Datei {
    /// Anzeigename und Dateiname beim Herunterladen
    pub name: String,
    /// Pfad auf dem Server, ausserhalb des Webverzeichnisses
    pub pfad: PathBuf,
}

impl Konfig {
    pub fn laden(pfad: &str) -> Result<Self> {
        let text = std::fs::read_to_string(pfad).with_context(|| format!("Konfiguration {pfad}"))?;
        let mut k: Konfig = toml::from_str(&text).with_context(|| format!("Konfiguration {pfad}"))?;
        k.pruefen()?;
        Ok(k)
    }

    pub fn pruefen(&mut self) -> Result<()> {
        self.glaeubiger.iban = rechnung::iban_pruefen(&self.glaeubiger.iban)?;
        self.server.basis_url = self.server.basis_url.trim_end_matches('/').to_string();
        if self.server.admin_passwort.len() < 12 {
            bail!("admin_passwort: mindestens 12 Zeichen");
        }
        for p in &self.produkte {
            if p.id.is_empty() || !p.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
                bail!("Produkt-ID {:?}: nur a-z, 0-9 und -", p.id);
            }
            if p.waehrung != "CHF" && p.waehrung != "EUR" {
                bail!("Produkt {}: Währung nur CHF oder EUR", p.id);
            }
            let v: f64 = p.preis.parse().with_context(|| format!("Produkt {}: Preis", p.id))?;
            if !(0.01..=999_999_999.99).contains(&v) {
                bail!("Produkt {}: Preis ausserhalb 0.01 bis 999999999.99", p.id);
            }
            if p.dateien.is_empty() {
                bail!("Produkt {}: keine Datei", p.id);
            }
            for d in &p.dateien {
                if !d.pfad.is_file() {
                    bail!("Produkt {}: Datei {} fehlt", p.id, d.pfad.display());
                }
            }
        }
        Ok(())
    }

    pub fn produkt(&self, id: &str) -> Option<&Produkt> {
        self.produkte.iter().find(|p| p.id == id)
    }

    pub fn leer(&self) -> bool {
        self.produkte.is_empty() && self.kataloge.is_empty()
    }
}

impl Produkt {
    pub fn preis_text(&self) -> String {
        format!("{} {}", self.waehrung, self.preis)
    }
}

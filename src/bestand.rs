// SPDX-License-Identifier: GPL-3.0-or-later
//
// Bestellungen in SQLite. Eine Bestellung trägt zwei Geheimnisse: den
// Rechnungs-Schlüssel (Link auf die QR-Rechnung, gleich nach der Bestellung)
// und den Download-Schlüssel (erst nach der Freigabe von Hand).

use std::path::Path;

use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

use crate::rechnung;

#[derive(Debug, Clone)]
pub struct Bestellung {
    pub id: i64,
    pub produkt: String,
    pub name: String,
    pub email: String,
    pub betrag: String,
    pub waehrung: String,
    pub referenz: String,
    pub rechnung_schluessel: String,
    pub erstellt: i64,
    pub bezahlt: Option<i64>,
    pub download_schluessel: Option<String>,
    pub gueltig_bis: Option<i64>,
    pub downloads: i64,
}

pub fn jetzt() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 32 zufällige Bytes als Hex: nicht zu erraten.
pub fn schluessel() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub struct Bestand {
    db: Connection,
}

const SPALTEN: &str = "id, produkt, name, email, betrag, waehrung, referenz, rechnung_schluessel, \
                       erstellt, bezahlt, download_schluessel, gueltig_bis, downloads";

fn zeile(r: &rusqlite::Row) -> rusqlite::Result<Bestellung> {
    Ok(Bestellung {
        id: r.get(0)?,
        produkt: r.get(1)?,
        name: r.get(2)?,
        email: r.get(3)?,
        betrag: r.get(4)?,
        waehrung: r.get(5)?,
        referenz: r.get(6)?,
        rechnung_schluessel: r.get(7)?,
        erstellt: r.get(8)?,
        bezahlt: r.get(9)?,
        download_schluessel: r.get(10)?,
        gueltig_bis: r.get(11)?,
        downloads: r.get(12)?,
    })
}

impl Bestand {
    pub fn oeffnen(ordner: &Path) -> Result<Self> {
        std::fs::create_dir_all(ordner).with_context(|| ordner.display().to_string())?;
        let db = Connection::open(ordner.join("kasse.sqlite"))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS bestellung (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                produkt TEXT NOT NULL,
                name TEXT NOT NULL,
                email TEXT NOT NULL,
                betrag TEXT NOT NULL,
                waehrung TEXT NOT NULL,
                referenz TEXT UNIQUE,
                rechnung_schluessel TEXT NOT NULL UNIQUE,
                erstellt INTEGER NOT NULL,
                bezahlt INTEGER,
                download_schluessel TEXT UNIQUE,
                gueltig_bis INTEGER,
                downloads INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        Ok(Bestand { db })
    }

    /// Legt eine Bestellung an; die SCOR-Referenz entsteht aus der laufenden
    /// Nummer, darum erst einfügen, dann die Referenz setzen.
    pub fn anlegen(&self, produkt: &str, name: &str, email: &str, betrag: &str, waehrung: &str) -> Result<Bestellung> {
        let rs = schluessel();
        self.db.execute(
            "INSERT INTO bestellung (produkt, name, email, betrag, waehrung, rechnung_schluessel, erstellt)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![produkt, name, email, betrag, waehrung, rs, jetzt()],
        )?;
        let id = self.db.last_insert_rowid();
        // Basis: «K» und die Nummer, mit Nullen auf 8 Stellen
        let referenz = rechnung::scor(&format!("K{id:08}"))?;
        self.db.execute("UPDATE bestellung SET referenz = ?1 WHERE id = ?2", params![referenz, id])?;
        self.nach_id(id)?.context("Bestellung verschwunden")
    }

    pub fn nach_id(&self, id: i64) -> Result<Option<Bestellung>> {
        Ok(self.db.query_row(&format!("SELECT {SPALTEN} FROM bestellung WHERE id = ?1"), [id], zeile).optional()?)
    }

    pub fn nach_rechnung(&self, s: &str) -> Result<Option<Bestellung>> {
        Ok(self
            .db
            .query_row(&format!("SELECT {SPALTEN} FROM bestellung WHERE rechnung_schluessel = ?1"), [s], zeile)
            .optional()?)
    }

    pub fn nach_download(&self, s: &str) -> Result<Option<Bestellung>> {
        Ok(self
            .db
            .query_row(&format!("SELECT {SPALTEN} FROM bestellung WHERE download_schluessel = ?1"), [s], zeile)
            .optional()?)
    }

    pub fn alle(&self) -> Result<Vec<Bestellung>> {
        let mut st = self.db.prepare(&format!("SELECT {SPALTEN} FROM bestellung ORDER BY id DESC"))?;
        let v = st.query_map([], zeile)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    }

    /// Freigabe von Hand: bezahlt, Download-Schlüssel, Ablaufdatum. Eine
    /// zweite Freigabe verlängert nur und behält den Schlüssel.
    pub fn freigeben(&self, id: i64, tage: i64) -> Result<Bestellung> {
        let b = self.nach_id(id)?.context("Bestellung nicht gefunden")?;
        let s = b.download_schluessel.unwrap_or_else(schluessel);
        let bis = jetzt() + tage * 86_400;
        self.db.execute(
            "UPDATE bestellung SET bezahlt = COALESCE(bezahlt, ?1), download_schluessel = ?2, gueltig_bis = ?3 WHERE id = ?4",
            params![jetzt(), s, bis, id],
        )?;
        self.nach_id(id)?.context("Bestellung verschwunden")
    }

    pub fn download_zaehlen(&self, id: i64) -> Result<()> {
        self.db.execute("UPDATE bestellung SET downloads = downloads + 1 WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bestellen_und_freigeben() {
        let ordner = std::env::temp_dir().join(format!("kasse-test-{}", schluessel()));
        let b = Bestand::oeffnen(&ordner).unwrap();
        let x = b.anlegen("bowen", "Anna", "anna@example.org", "50.00", "CHF").unwrap();
        assert!(rechnung::scor_gueltig(&x.referenz), "{}", x.referenz);
        assert!(x.bezahlt.is_none() && x.download_schluessel.is_none());
        let f = b.freigeben(x.id, 30).unwrap();
        let s = f.download_schluessel.clone().unwrap();
        assert_eq!(s.len(), 64);
        let f2 = b.freigeben(x.id, 30).unwrap();
        assert_eq!(f2.download_schluessel.unwrap(), s, "zweite Freigabe behält den Schlüssel");
        assert_eq!(b.nach_download(&s).unwrap().unwrap().id, x.id);
        std::fs::remove_dir_all(ordner).ok();
    }
}

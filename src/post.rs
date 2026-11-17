// SPDX-License-Identifier: GPL-3.0-or-later
//
// Mails an Käufer: die Rechnung (mit QR-Rechnung als PDF im Anhang) und nach
// der Freigabe den Download-Link. Versand über sendmail - auf ganglion.ch ist
// das msmtp mit dem Google-Workspace-Relay, wie beim Kontaktformular. Ist
// `mail.ordner` gesetzt, landen die Mails als .eml dort (Tests, lokal).
//
// Fehler beim Senden werden gemeldet, nie verschluckt: beim Kontaktformular
// von ganglion.ch blieb das 2026 monatelang unbemerkt.

use anyhow::{Context, Result};
use lettre::message::{header::ContentType, Attachment, Mailbox, MultiPart, SinglePart};
use lettre::{Message, SendmailTransport, Transport};

use crate::konfig::Mail;

pub struct Brief {
    pub an: String,
    pub betreff: String,
    pub text: String,
    /// (Dateiname, PDF-Bytes)
    pub anhang: Option<(String, Vec<u8>)>,
}

pub fn senden(k: &Mail, b: &Brief) -> Result<()> {
    let von: Mailbox = k.absender.parse().context("mail.absender")?;
    let an: Mailbox = b.an.parse().with_context(|| format!("Empfänger {:?}", b.an))?;
    let mut m = Message::builder().from(von).to(an).subject(b.betreff.replace(['\r', '\n'], " "));
    if let Some(kopie) = &k.kopie {
        m = m.bcc(kopie.parse().context("mail.kopie")?);
    }
    let text = SinglePart::builder().header(ContentType::TEXT_PLAIN).body(b.text.clone());
    let nachricht = match &b.anhang {
        Some((name, daten)) => m.multipart(
            MultiPart::mixed()
                .singlepart(text)
                .singlepart(Attachment::new(name.clone()).body(daten.clone(), ContentType::parse("application/pdf")?)),
        )?,
        None => m.singlepart(text)?,
    };
    match &k.ordner {
        Some(ordner) => {
            std::fs::create_dir_all(ordner)?;
            let datei = ordner.join(format!("{}-{}.eml", crate::bestand::jetzt(), crate::bestand::schluessel()));
            std::fs::write(&datei, nachricht.formatted())?;
        }
        None => SendmailTransport::new_with_command(&k.sendmail)
            .send(&nachricht)
            .with_context(|| format!("Mail an {} über {}", b.an, k.sendmail))?,
    }
    Ok(())
}

/// Einfache Prüfung: genau ein @, Punkt im Domainteil, keine Leer- oder
/// Steuerzeichen; den Rest erledigt das Mailbox-Parsing von lettre.
pub fn adresse_gueltig(s: &str) -> bool {
    let s = s.trim();
    let Some((lokal, domain)) = s.split_once('@') else { return false };
    !lokal.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && s.len() <= 254
        && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>' || c == ',')
        && s.matches('@').count() == 1
        && s.parse::<Mailbox>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adressen() {
        assert!(adresse_gueltig("anna@example.org"));
        assert!(!adresse_gueltig("anna@example"));
        assert!(!adresse_gueltig("anna example@x.org"));
        assert!(!adresse_gueltig("a@b.org\r\nBcc: x@y.org"));
        assert!(!adresse_gueltig("a@b.org, c@d.org"));
    }
}

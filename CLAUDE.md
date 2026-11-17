# CLAUDE.md

rustpay (Programm `rustpay`, intern «Kasse»).

Verkauf von PDFs gegen Schweizer QR-Rechnung (siehe README.md). Kommentare,
Texte und Commit-Messages auf Deutsch, Schweizer Rechtschreibung (ss statt ß).

## Aufbau

- `src/main.rs` – axum-Server: `/p/<id>` (Bestellung), `/r/<schlüssel>`
  (Rechnung), `/d/<schlüssel>[/<nr>]` (Download), `/admin`,
  `/admin/freigeben/<id>`; Tests mit `tower::ServiceExt::oneshot`.
- `src/rechnung.rs` – QR-Rechnung nach SIX IG 2.3, aus
  `adhs-expert/src/qrrechnung.rs` übernommen, hier mit SCOR-Referenz. Bei
  Änderungen beide Stellen nachziehen. Prüfen: PDF mit `pdftoppm -r 300`
  rendern und mit zxing-cpp lesen (`uv run --with zxing-cpp --with pillow`).
- `src/bestand.rs` – SQLite (`rusqlite`, bundled): Bestellungen, Schlüssel.
- `src/post.rs` – Mails über sendmail (lettre) oder als .eml in einen Ordner.
- `src/konfig.rs` – TOML-Konfiguration, wird beim Start geprüft (IBAN,
  Produkte, Dateien vorhanden).

## Regeln

- **Keine Kontodaten, Passwörter oder Mailadressen von Personen im Code
  oder im Repository**; nur in `kasse.toml` (gitignored). Testdaten:
  Beispiel-IBAN CH9300762011623852957, `example.org`.
- Freigabe ist **von Hand** (Wunsch des Benutzers, November 2026: «beginnen
  wir mal von Hand»). Der automatische Abgleich über camt kommt später; die
  SCOR-Referenz ist dafür schon da.
- Mailfehler nie verschlucken, sondern melden (Lehre aus dem
  ganglion.ch-Kontaktformular).
- Neue Abhängigkeiten müssen GPL-3.0-verträglich sein.

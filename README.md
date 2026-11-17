# kasse

Verkauf von PDFs gegen **Schweizer QR-Rechnung**, in Rust. Zuerst für die
deutsche Übersetzung von Murray Bowens «Familientherapie in der klinischen
Praxis» (adhs.expert / ganglion.ch), gedacht für alle PDFs von adhs.expert.

## Ablauf

1. **Produktseite** `/p/<id>`: Käufer gibt Name und Mailadresse an.
2. Die Kasse legt eine **Bestellung** mit eigener SCOR-Referenz (ISO 11649,
   `RF.. K00000001`) an, erzeugt die **QR-Rechnung** (SIX IG 2.3, Zahlteil mit
   Empfangsschein) und **mailt** sie mit PDF-Anhang; Kopie an `mail.kopie`.
3. **Freigabe von Hand**: Zahlung im E-Banking anhand der Referenz prüfen,
   auf `/admin` (HTTP Basic Auth) «bezahlt – Link senden» klicken.
4. Der Käufer erhält einen **Download-Link** (`/d/<64 Hex-Zeichen>`),
   `gueltig_tage` lang gültig. Die PDFs liegen ausserhalb des
   Webverzeichnisses; die Kasse liefert sie aus und zählt die Downloads.

Die SCOR-Referenz erlaubt später einen automatischen Abgleich mit
camt.053/054-Auszügen (etwa über den EBICS-Client von taxtsueri).

## Bauen und testen

```bash
cargo build --release
cargo test --release            # u.a. ganzer Ablauf Bestellung → Freigabe → Download
cp kasse.toml.beispiel kasse.toml   # ausfüllen
./target/release/kasse --konfig kasse.toml --pruefen
./target/release/kasse --konfig kasse.toml
```

## Betrieb auf ganglion.ch

- Binary nach `/usr/local/bin/kasse`, Konfiguration `/etc/kasse/kasse.toml`
  (Rechte 0640, Benutzer `kasse`), Datenbank in `/var/lib/kasse`.
- `betrieb/kasse.service` (systemd) und `betrieb/apache-kasse.conf`
  (`ProxyPass /kasse/ http://127.0.0.1:3001/`).
- Mail über `/usr/sbin/sendmail` (msmtp → Google-Workspace-Relay, wie das
  Kontaktformular von ganglion.ch). Der Absender muss zu einer Domain des
  Workspace passen.
- Schriften: auf Ubuntu `fonts-liberation` (Liberation Sans).

## Sicherheit

- Download- und Rechnungs-Schlüssel: 32 Zufallsbytes aus dem Betriebssystem.
- Admin: Basic Auth, zeitkonstanter Vergleich; Freigaben nur mit passendem
  `Origin`/`Referer` (gegen Formulare von fremden Seiten).
- Mailadressen werden geprüft (keine Zeilenumbrüche, nur eine Adresse).
- `kasse.toml` (Admin-Passwort) gehört nicht ins Repository.

## Lizenz

GPL-3.0-or-later.

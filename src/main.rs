// SPDX-License-Identifier: GPL-3.0-or-later
//
// kasse - Verkauf von PDFs gegen Schweizer QR-Rechnung.
//
// Ablauf (Wunsch des Benutzers, November 2026, zuerst für die deutsche
// Übersetzung von Murray Bowens «Familientherapie in der klinischen Praxis»):
//
//   1. Produktseite /p/<id>: Name und Mailadresse eingeben.
//   2. Die Kasse legt eine Bestellung mit eigener SCOR-Referenz an, erzeugt
//      die QR-Rechnung und mailt sie (PDF im Anhang, Kopie an mail.kopie).
//   3. Eingang der Zahlung von Hand prüfen und auf /admin «bezahlt» klicken
//      (Variante «von Hand»; die SCOR-Referenz erlaubt später einen
//      automatischen Abgleich mit camt-Auszügen).
//   4. Der Käufer bekommt einen Download-Link, `gueltig_tage` lang gültig;
//      die PDFs liegen ausserhalb des Webverzeichnisses, die Kasse liefert sie.
//
//   kasse --konfig /pfad/kasse.toml          # Server starten
//   kasse --konfig /pfad/kasse.toml --pruefen # nur Konfiguration prüfen
//
// Auf ganglion.ch hinter Apache: ProxyPass /kasse/ http://127.0.0.1:3001/

mod bestand;
mod konfig;
mod post;
mod rechnung;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;

use bestand::{Bestand, Bestellung};
use konfig::{Konfig, Produkt};

struct App {
    k: Konfig,
    bestand: Mutex<Bestand>,
    schriften: rechnung::Schriften,
}

type Zustand = Arc<App>;

fn main() -> Result<()> {
    let mut pfad = "kasse.toml".to_string();
    let mut nur_pruefen = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--konfig" => pfad = args.next().context("--konfig braucht einen Pfad")?,
            "--pruefen" => nur_pruefen = true,
            "-h" | "--help" => {
                println!("kasse --konfig kasse.toml [--pruefen]");
                return Ok(());
            }
            _ => anyhow::bail!("unbekannt: {a}"),
        }
    }
    let k = Konfig::laden(&pfad)?;
    let schriften = rechnung::Schriften::laden(&k.server.schrift_normal, &k.server.schrift_fett)?;
    if nur_pruefen {
        println!("Konfiguration in Ordnung: {} Produkt(e), Gläubiger {}", k.produkte.len(), k.glaeubiger.name);
        return Ok(());
    }
    let bestand = Bestand::oeffnen(&k.server.daten)?;
    let adresse = k.server.adresse.clone();
    let app = Arc::new(App { k, bestand: Mutex::new(bestand), schriften });
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        let lauscher = tokio::net::TcpListener::bind(&adresse).await.with_context(|| adresse.clone())?;
        println!("kasse läuft auf {adresse}");
        axum::serve(lauscher, router(app)).await?;
        Ok(())
    })
}

fn router(app: Zustand) -> Router {
    Router::new()
        .route("/", get(start))
        .route("/p/{id}", get(produktseite).post(bestellen))
        .route("/r/{schluessel}", get(rechnung_pdf))
        .route("/d/{schluessel}", get(download_seite))
        .route("/d/{schluessel}/{nr}", get(download_datei))
        .route("/admin", get(admin))
        .route("/admin/freigeben/{id}", post(freigeben))
        .with_state(app)
}

// ------------------------------------------------------------------ Seiten

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn seite(titel: &str, inhalt: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex\"><title>{}</title><style>\
         body{{font-family:Helvetica,Arial,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;color:#222;line-height:1.5}}\
         h1{{font-size:1.5rem}} label{{display:block;margin-top:.8rem}}\
         input[type=text],input[type=email]{{width:100%;padding:.5rem;font-size:1rem;box-sizing:border-box}}\
         button{{margin-top:1rem;padding:.6rem 1.2rem;font-size:1rem;cursor:pointer}}\
         table{{border-collapse:collapse;width:100%;font-size:.9rem}} td,th{{border-bottom:1px solid #ddd;padding:.3rem;text-align:left;vertical-align:top}}\
         .hinweis{{background:#f4f4f4;padding:.8rem 1rem;border-radius:4px}} .falle{{position:absolute;left:-9999px}}\
         </style></head><body>{}</body></html>",
        esc(titel),
        inhalt
    ))
}

fn fehler(code: StatusCode, text: &str) -> Response {
    (code, seite("Fehler", &format!("<h1>{}</h1><p>{}</p>", code.as_u16(), esc(text)))).into_response()
}

fn intern(e: anyhow::Error) -> Response {
    eprintln!("Fehler: {e:#}");
    fehler(StatusCode::INTERNAL_SERVER_ERROR, "Da ist etwas schiefgegangen. Bitte später noch einmal versuchen.")
}

/// Unix-Zeit → «17.11.2026» (UTC; für Tagesangaben genau genug).
fn datum(t: i64) -> String {
    let tage = t.div_euclid(86_400);
    // Howard Hinnant, civil_from_days
    let z = tage + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{d:02}.{m:02}.{y}")
}

async fn start(State(app): State<Zustand>) -> Html<String> {
    let liste: String = app
        .k
        .produkte
        .iter()
        .map(|p| format!("<li><a href=\"{}/p/{}\">{}</a> – {}</li>", app.k.server.basis_url, p.id, esc(&p.titel), esc(&p.preis_text())))
        .collect();
    seite("Kasse", &format!("<h1>Bestellen</h1><ul>{liste}</ul>"))
}

async fn produktseite(State(app): State<Zustand>, Path(id): Path<String>) -> Response {
    let Some(p) = app.k.produkt(&id) else { return fehler(StatusCode::NOT_FOUND, "Dieses Angebot gibt es nicht.") };
    let dateien: String = p.dateien.iter().map(|d| format!("<li>{}</li>", esc(&d.name))).collect();
    seite(
        &p.titel,
        &format!(
            "<h1>{}</h1><p>{}</p><p><strong>Preis: {}</strong></p><p>Enthalten:</p><ul>{dateien}</ul>\
             <div class=\"hinweis\">Sie erhalten die Rechnung mit Schweizer QR-Code per Mail. Sobald die Zahlung \
             eingegangen ist, schicken wir Ihnen einen Link zum Herunterladen ({} Tage gültig).</div>\
             <form method=\"post\" action=\"{}/p/{}\">\
             <label>Name<input type=\"text\" name=\"name\" required maxlength=\"100\" autocomplete=\"name\"></label>\
             <label>E-Mail<input type=\"email\" name=\"email\" required maxlength=\"254\" autocomplete=\"email\"></label>\
             <label class=\"falle\">Webseite<input type=\"text\" name=\"webseite\" tabindex=\"-1\" autocomplete=\"off\"></label>\
             <button type=\"submit\">Rechnung anfordern</button></form>",
            esc(&p.titel),
            esc(&p.beschreibung),
            esc(&p.preis_text()),
            p.gueltig_tage,
            app.k.server.basis_url,
            p.id
        ),
    )
    .into_response()
}

#[derive(serde::Deserialize)]
struct BestellFormular {
    name: String,
    email: String,
    #[serde(default)]
    webseite: String,
}

fn rechnung_erzeugen(app: &App, p: &Produkt, b: &Bestellung) -> Result<Vec<u8>> {
    let r = rechnung::Rechnung {
        glaeubiger: &app.k.glaeubiger,
        betrag: b.betrag.clone(),
        waehrung: b.waehrung.clone(),
        referenz: b.referenz.clone(),
        mitteilung: format!("{} (Bestellung {})", p.titel, b.id),
    };
    rechnung::pdf(&r, &app.schriften)
}

async fn bestellen(State(app): State<Zustand>, Path(id): Path<String>, Form(f): Form<BestellFormular>) -> Response {
    let Some(p) = app.k.produkt(&id).cloned() else { return fehler(StatusCode::NOT_FOUND, "Dieses Angebot gibt es nicht.") };
    let name = f.name.trim().to_string();
    let email = f.email.trim().to_string();
    if !f.webseite.is_empty() {
        // Falle für Formular-Roboter: so tun, als sei alles gut
        return seite("Danke", "<h1>Danke</h1><p>Die Rechnung ist unterwegs.</p>").into_response();
    }
    if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return fehler(StatusCode::BAD_REQUEST, "Bitte einen Namen angeben.");
    }
    if !post::adresse_gueltig(&email) {
        return fehler(StatusCode::BAD_REQUEST, "Diese Mailadresse ist nicht gültig.");
    }
    let app2 = app.clone();
    let erg = tokio::task::spawn_blocking(move || -> Result<Bestellung> {
        let b = app2.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?.anlegen(&p.id, &name, &email, &p.preis, &p.waehrung)?;
        let pdf = rechnung_erzeugen(&app2, &p, &b)?;
        let link = format!("{}/r/{}", app2.k.server.basis_url, b.rechnung_schluessel);
        post::senden(
            &app2.k.mail,
            &post::Brief {
                an: b.email.clone(),
                betreff: format!("Rechnung: {}", p.titel),
                text: format!(
                    "Guten Tag {}\n\nvielen Dank für Ihre Bestellung «{}».\n\n\
                     Betrag: {} {}\nReferenz: {}\n\n\
                     Die QR-Rechnung liegt dieser Mail bei; Sie finden sie auch hier:\n{link}\n\n\
                     Bitte zahlen Sie mit dem QR-Code oder mit der Referenz. Sobald die Zahlung \
                     eingegangen ist, erhalten Sie einen Link zum Herunterladen ({} Tage gültig).\n\n\
                     Freundliche Grüsse\n{}\n",
                    b.name, p.titel, b.waehrung, b.betrag, b.referenz, p.gueltig_tage, app2.k.glaeubiger.name
                ),
                anhang: Some((format!("Rechnung_{}.pdf", b.id), pdf)),
            },
        )?;
        Ok(b)
    })
    .await;
    match erg {
        Ok(Ok(b)) => seite(
            "Rechnung unterwegs",
            &format!(
                "<h1>Danke für Ihre Bestellung</h1><p>Die Rechnung ist an <strong>{}</strong> unterwegs. \
                 Sie können sie auch gleich hier öffnen und mit Ihrer Banking-App bezahlen:</p>\
                 <p><a href=\"{}/r/{}\" target=\"_blank\" rel=\"noopener\">QR-Rechnung öffnen (PDF)</a></p>\
                 <p>Referenz: {}</p><p>Sobald die Zahlung eingegangen ist, erhalten Sie den Link zum Herunterladen per Mail.</p>",
                esc(&b.email),
                app.k.server.basis_url,
                b.rechnung_schluessel,
                esc(&b.referenz)
            ),
        )
        .into_response(),
        Ok(Err(e)) => intern(e),
        Err(e) => intern(e.into()),
    }
}

async fn rechnung_pdf(State(app): State<Zustand>, Path(s): Path<String>) -> Response {
    let app2 = app.clone();
    let erg = tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
        let Some(b) = app2.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?.nach_rechnung(&s)? else { return Ok(None) };
        let p = app2.k.produkt(&b.produkt).cloned().context("Produkt nicht mehr im Angebot")?;
        Ok(Some(rechnung_erzeugen(&app2, &p, &b)?))
    })
    .await;
    match erg {
        Ok(Ok(Some(pdf))) => (
            [(header::CONTENT_TYPE, "application/pdf"), (header::CONTENT_DISPOSITION, "inline; filename=\"Rechnung.pdf\"")],
            pdf,
        )
            .into_response(),
        Ok(Ok(None)) => fehler(StatusCode::NOT_FOUND, "Diese Rechnung gibt es nicht."),
        Ok(Err(e)) => intern(e),
        Err(e) => intern(e.into()),
    }
}

/// Bestellung zu einem Download-Schlüssel, nur wenn freigegeben und gültig.
fn gueltige_bestellung(app: &App, s: &str) -> Result<std::result::Result<(Bestellung, Produkt), &'static str>> {
    let Some(b) = app.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?.nach_download(s)? else {
        return Ok(Err("Dieser Link ist nicht gültig."));
    };
    if b.gueltig_bis.unwrap_or(0) < bestand::jetzt() {
        return Ok(Err("Dieser Link ist abgelaufen. Bitte melden Sie sich, wir schicken gern einen neuen."));
    }
    let Some(p) = app.k.produkt(&b.produkt).cloned() else { return Ok(Err("Dieses Angebot gibt es nicht mehr.")) };
    Ok(Ok((b, p)))
}

async fn download_seite(State(app): State<Zustand>, Path(s): Path<String>) -> Response {
    match gueltige_bestellung(&app, &s) {
        Ok(Ok((b, p))) => {
            let liste: String = p
                .dateien
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    format!(
                        "<li><a href=\"{}/d/{}/{}\" target=\"_blank\" rel=\"noopener\">{}</a></li>",
                        app.k.server.basis_url,
                        s,
                        i + 1,
                        esc(&d.name)
                    )
                })
                .collect();
            seite(
                &p.titel,
                &format!(
                    "<h1>{}</h1><p>Guten Tag {}, hier sind Ihre Dateien:</p><ul>{liste}</ul>\
                     <p>Der Link gilt bis {}.</p>",
                    esc(&p.titel),
                    esc(&b.name),
                    datum(b.gueltig_bis.unwrap_or(0))
                ),
            )
            .into_response()
        }
        Ok(Err(text)) => fehler(StatusCode::FORBIDDEN, text),
        Err(e) => intern(e),
    }
}

fn dateiname_ascii(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect()
}

fn prozent(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b".-_".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

async fn download_datei(State(app): State<Zustand>, Path((s, nr)): Path<(String, usize)>) -> Response {
    let (b, p) = match gueltige_bestellung(&app, &s) {
        Ok(Ok(x)) => x,
        Ok(Err(text)) => return fehler(StatusCode::FORBIDDEN, text),
        Err(e) => return intern(e),
    };
    let Some(d) = nr.checked_sub(1).and_then(|i| p.dateien.get(i)) else {
        return fehler(StatusCode::NOT_FOUND, "Diese Datei gibt es nicht.");
    };
    let datei = match tokio::fs::File::open(&d.pfad).await {
        Ok(f) => f,
        Err(e) => return intern(anyhow::anyhow!("{}: {e}", d.pfad.display())),
    };
    let laenge = datei.metadata().await.map(|m| m.len()).unwrap_or(0);
    if let Ok(g) = app.bestand.lock() {
        let _ = g.download_zaehlen(b.id);
    }
    let name = if d.name.to_lowercase().ends_with(".pdf") { d.name.clone() } else { format!("{}.pdf", d.name) };
    let koerper = Body::from_stream(tokio_util::io::ReaderStream::new(datei));
    Response::builder()
        .header(header::CONTENT_TYPE, "application/pdf")
        .header(header::CONTENT_LENGTH, laenge)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"; filename*=UTF-8''{}", dateiname_ascii(&name), prozent(&name)),
        )
        .body(koerper)
        .unwrap_or_else(|e| intern(e.into()))
}

// ------------------------------------------------------------------ Admin

/// HTTP Basic Auth gegen admin_benutzer/admin_passwort (zeitkonstant verglichen).
fn angemeldet(app: &App, h: &HeaderMap) -> bool {
    let Some(wert) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else { return false };
    let Some(b64) = wert.strip_prefix("Basic ") else { return false };
    let Ok(roh) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else { return false };
    let erwartet = format!("{}:{}", app.k.server.admin_benutzer, app.k.server.admin_passwort);
    roh.len() == erwartet.len() && roh.iter().zip(erwartet.as_bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
}

fn anmelden() -> Response {
    (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, "Basic realm=\"kasse\", charset=\"UTF-8\"")], "Anmeldung nötig")
        .into_response()
}

/// Gegen Formulare von fremden Seiten: kommt ein Origin- oder Referer-Kopf
/// mit, muss er zur basis_url passen.
fn gleiche_herkunft(app: &App, h: &HeaderMap) -> bool {
    let basis = &app.k.server.basis_url;
    let herkunft = basis.split('/').take(3).collect::<Vec<_>>().join("/");
    match (h.get(header::ORIGIN), h.get(header::REFERER)) {
        (Some(o), _) => o.to_str().is_ok_and(|o| o == herkunft),
        (None, Some(r)) => r.to_str().is_ok_and(|r| r.starts_with(basis.as_str())),
        (None, None) => true,
    }
}

async fn admin(State(app): State<Zustand>, h: HeaderMap) -> Response {
    if !angemeldet(&app, &h) {
        return anmelden();
    }
    let alle = match app.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre")).and_then(|g| g.alle()) {
        Ok(v) => v,
        Err(e) => return intern(e),
    };
    let zeilen: String = alle
        .iter()
        .map(|b| {
            let status = match (b.bezahlt, b.gueltig_bis) {
                (Some(t), Some(bis)) => format!("bezahlt {}<br>Link bis {}", datum(t), datum(bis)),
                _ => "offen".into(),
            };
            let knopf = if b.bezahlt.is_some() { "Link erneut senden" } else { "bezahlt – Link senden" };
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}<br>{}</td><td>{} {}</td><td>{}</td><td>{}</td><td>{}</td>\
                 <td><form method=\"post\" action=\"{}/admin/freigeben/{}\" onsubmit=\"return confirm('Zahlung von {} {} erhalten?')\">\
                 <button type=\"submit\">{}</button></form></td></tr>",
                b.id,
                datum(b.erstellt),
                esc(&b.produkt),
                esc(&b.name),
                esc(&b.email),
                esc(&b.waehrung),
                esc(&b.betrag),
                esc(&b.referenz),
                status,
                b.downloads,
                app.k.server.basis_url,
                b.id,
                esc(&b.waehrung),
                esc(&b.betrag),
                knopf
            )
        })
        .collect();
    seite(
        "Kasse – Bestellungen",
        &format!(
            "<h1>Bestellungen</h1><p>Zahlung im E-Banking anhand der Referenz prüfen, dann «bezahlt» klicken: \
             der Käufer erhält den Download-Link per Mail.</p>\
             <table><tr><th>Nr.</th><th>Datum</th><th>Produkt</th><th>Käufer</th><th>Betrag</th><th>Referenz</th>\
             <th>Status</th><th>Downloads</th><th></th></tr>{zeilen}</table>"
        ),
    )
    .into_response()
}

async fn freigeben(State(app): State<Zustand>, h: HeaderMap, Path(id): Path<i64>) -> Response {
    if !angemeldet(&app, &h) {
        return anmelden();
    }
    if !gleiche_herkunft(&app, &h) {
        return fehler(StatusCode::FORBIDDEN, "Fremde Herkunft.");
    }
    let app2 = app.clone();
    let erg = tokio::task::spawn_blocking(move || -> Result<()> {
        let b0 = app2.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?.nach_id(id)?.context("Bestellung nicht gefunden")?;
        let p = app2.k.produkt(&b0.produkt).cloned().context("Produkt nicht mehr im Angebot")?;
        let b = app2.bestand.lock().map_err(|_| anyhow::anyhow!("Sperre"))?.freigeben(id, p.gueltig_tage)?;
        let link = format!("{}/d/{}", app2.k.server.basis_url, b.download_schluessel.clone().unwrap_or_default());
        post::senden(
            &app2.k.mail,
            &post::Brief {
                an: b.email.clone(),
                betreff: format!("Download: {}", p.titel),
                text: format!(
                    "Guten Tag {}\n\nIhre Zahlung für «{}» ist eingegangen, vielen Dank.\n\n\
                     Hier können Sie die Dateien herunterladen:\n{link}\n\n\
                     Der Link gilt bis {}. Bitte geben Sie ihn nicht weiter.\n\n\
                     Freundliche Grüsse\n{}\n",
                    b.name,
                    p.titel,
                    datum(b.gueltig_bis.unwrap_or(0)),
                    app2.k.glaeubiger.name
                ),
                anhang: None,
            },
        )
    })
    .await;
    match erg {
        Ok(Ok(())) => Redirect::to(&format!("{}/admin", app.k.server.basis_url)).into_response(),
        Ok(Err(e)) => intern(e),
        Err(e) => intern(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn testapp() -> (Zustand, std::path::PathBuf) {
        let ordner = std::env::temp_dir().join(format!("kasse-web-{}", bestand::schluessel()));
        std::fs::create_dir_all(&ordner).unwrap();
        let pdf = ordner.join("buch.pdf");
        std::fs::write(&pdf, b"%PDF-1.4 Testinhalt").unwrap();
        let toml = format!(
            r#"
            [server]
            adresse = "127.0.0.1:0"
            basis_url = "https://beispiel.ch/kasse"
            daten = "{d}"
            admin_benutzer = "admin"
            admin_passwort = "sehr-geheimes-passwort"
            [mail]
            absender = "Kasse <kasse@beispiel.ch>"
            ordner = "{d}/post"
            [glaeubiger]
            iban = "CH93 0076 2011 6238 5295 7"
            name = "Muster AG"
            strasse = "Weg"
            hausnr = "1"
            plz = "8000"
            ort = "Zürich"
            uid = "CHE-123.456.789 MWST"
            [[produkt]]
            id = "buch"
            titel = "Ein Buch"
            preis = "50.00"
            [[produkt.datei]]
            name = "Buch.pdf"
            pfad = "{p}"
            "#,
            d = ordner.display(),
            p = pdf.display()
        );
        let mut k: Konfig = toml::from_str(&toml).unwrap();
        k.pruefen().unwrap();
        let schriften = rechnung::Schriften::laden(&k.server.schrift_normal, &k.server.schrift_fett).unwrap();
        let bestand = Bestand::oeffnen(&k.server.daten).unwrap();
        (Arc::new(App { k, bestand: Mutex::new(bestand), schriften }), ordner)
    }

    async fn holen(app: &Zustand, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let r = router(app.clone()).oneshot(req).await.unwrap();
        let s = r.status();
        let h = r.headers().clone();
        let b = r.into_body().collect().await.unwrap().to_bytes().to_vec();
        (s, h, b)
    }

    fn basic() -> String {
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("admin:sehr-geheimes-passwort"))
    }

    #[tokio::test]
    async fn ganzer_ablauf() {
        let (app, ordner) = testapp();
        // Bestellung
        let req = Request::post("/p/buch")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from("name=Anna+M%C3%BCller&email=anna%40example.org"))
            .unwrap();
        let (s, _, b) = holen(&app, req).await;
        assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
        let mails: Vec<_> = std::fs::read_dir(ordner.join("post")).unwrap().collect();
        assert_eq!(mails.len(), 1, "Rechnungsmail");
        let b = app.bestand.lock().unwrap().nach_id(1).unwrap().unwrap();
        assert!(rechnung::scor_gueltig(&b.referenz));
        // Rechnung abrufbar, Download noch nicht
        let (s, h, pdf) = holen(&app, Request::get(format!("/r/{}", b.rechnung_schluessel)).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(h[header::CONTENT_TYPE], "application/pdf");
        assert!(pdf.starts_with(b"%PDF"));
        // Admin ohne Anmeldung
        let (s, _, _) = holen(&app, Request::get("/admin").body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        // Freigabe von fremder Seite abgewiesen
        let req = Request::post("/admin/freigeben/1")
            .header(header::AUTHORIZATION, basic())
            .header(header::ORIGIN, "https://boese.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(holen(&app, req).await.0, StatusCode::FORBIDDEN);
        // Freigabe
        let req = Request::post("/admin/freigeben/1")
            .header(header::AUTHORIZATION, basic())
            .header(header::ORIGIN, "https://beispiel.ch")
            .body(Body::empty())
            .unwrap();
        assert_eq!(holen(&app, req).await.0, StatusCode::SEE_OTHER);
        assert_eq!(std::fs::read_dir(ordner.join("post")).unwrap().count(), 2, "Download-Mail");
        let s_dl = app.bestand.lock().unwrap().nach_id(1).unwrap().unwrap().download_schluessel.unwrap();
        // Download
        let (s, h, inhalt) = holen(&app, Request::get(format!("/d/{s_dl}/1")).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::OK);
        assert!(h[header::CONTENT_DISPOSITION].to_str().unwrap().contains("Buch.pdf"));
        assert_eq!(inhalt, b"%PDF-1.4 Testinhalt");
        assert_eq!(app.bestand.lock().unwrap().nach_id(1).unwrap().unwrap().downloads, 1);
        // falscher Schlüssel, falsche Nummer
        assert_eq!(holen(&app, Request::get("/d/falsch/1").body(Body::empty()).unwrap()).await.0, StatusCode::FORBIDDEN);
        assert_eq!(holen(&app, Request::get(format!("/d/{s_dl}/9")).body(Body::empty()).unwrap()).await.0, StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(ordner).ok();
    }

    #[tokio::test]
    async fn falsche_eingaben() {
        let (app, ordner) = testapp();
        let req = |body: &'static str| {
            Request::post("/p/buch")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap()
        };
        assert_eq!(holen(&app, req("name=A&email=keine")).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(holen(&app, req("name=&email=a%40b.ch")).await.0, StatusCode::BAD_REQUEST);
        // Roboter-Falle: Antwort ok, aber keine Bestellung
        assert_eq!(holen(&app, req("name=A&email=a%40b.ch&webseite=x")).await.0, StatusCode::OK);
        assert!(app.bestand.lock().unwrap().alle().unwrap().is_empty());
        assert_eq!(holen(&app, Request::get("/p/gibtsnicht").body(Body::empty()).unwrap()).await.0, StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(ordner).ok();
    }

    #[test]
    fn datum_umrechnen() {
        assert_eq!(datum(0), "01.01.1970");
        assert_eq!(datum(1_794_960_000), "18.11.2026");
    }
}

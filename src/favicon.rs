//! Domain icons.
//!
//! A domain is a website as well as a mailbox, so Receive asks it for its
//! favicon and draws that instead of a monogram or a plain glyph. These are the
//! only requests Receive makes to anything other than Resend, and they carry no
//! mail: each one asks a fixed path for an image and nothing else.
//!
//! Only [`CANDIDATES`] are asked for, and only of the domain itself. Nothing
//! here reads a page. The reply has to open like an image and stay under
//! [`LARGEST`] to be kept, so a site answering a missing icon with its home page
//! and a 200 is turned down rather than saved.
//!
//! Icons are kept as files in the application data directory, so a domain is
//! asked at most once a month. A domain with no usable icon is recorded too, so
//! a site without one is not asked again on every launch.

use anyhow::Result;
use reqwest::blocking::Client;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

/// Where a domain's icon is looked for, in order.
///
/// Only these four paths, and only on the domain itself. Receive does not read
/// a site's pages looking for an icon it declares: that would mean downloading
/// and parsing arbitrary HTML from every domain that writes to you, which is a
/// far wider surface than asking a fixed path for an image.
const CANDIDATES: [&str; 4] = [
    "/favicon.ico",
    "/favicon.svg",
    "/favicon.png",
    "/apple-touch-icon.png",
];

/// The few large mail providers that serve nothing at those paths, with the
/// address of their icon written down instead.
///
/// Every URL here was checked by hand and still goes through the same size and
/// magic-byte checks as any other. Most providers need no entry: Outlook,
/// iCloud, Proton, Fastmail, Zoho, Yahoo, AOL, Yandex, Mail.ru, QQ, Libero and
/// the rest all answer `/favicon.ico` themselves. Add a domain here when it
/// turns out not to.
const KNOWN: [(&str, &str); 5] = [
    (
        "gmail.com",
        "https://ssl.gstatic.com/ui/v1/icons/mail/rfr/gmail.ico",
    ),
    (
        "googlemail.com",
        "https://ssl.gstatic.com/ui/v1/icons/mail/rfr/gmail.ico",
    ),
    ("hotmail.com", "https://outlook.com/owa/favicon.ico"),
    ("live.com", "https://outlook.com/owa/favicon.ico"),
    ("gmx.com", "https://www.gmx.net/favicon.ico"),
];
/// How long an icon is used before the domain is asked again.
const KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a domain is left alone after it had no icon to give.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Favicons are small. Anything larger is not one, and is not worth reading.
const LARGEST: u64 = 512 * 1024;

/// A background thread that turns domain names into icon files.
///
/// Requests that fail are simply never answered; the rail keeps its monogram.
pub struct Icons {
    pub wanted: Sender<String>,
    pub found: Receiver<(String, PathBuf)>,
}

impl Icons {
    pub fn start(data_dir: PathBuf) -> Self {
        let (wanted, requests) = mpsc::channel::<String>();
        let (results, found) = mpsc::channel();
        std::thread::spawn(move || {
            let directory = data_dir.join("icons");
            if fs::create_dir_all(&directory).is_err() {
                return;
            }
            let Ok(client) = client() else { return };
            while let Ok(domain) = requests.recv() {
                if let Ok(Some(icon)) = icon_for(&client, &directory, &domain)
                    && results.send((domain, icon)).is_err()
                {
                    return;
                }
            }
        });
        Self { wanted, found }
    }
}

fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("Receive/0.1.0")
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        // Apex domains commonly redirect to `www`, and plain sites to HTTPS.
        .redirect(reqwest::redirect::Policy::limited(4))
        .build()?)
}

/// The domain's icon on disk, fetching it first if what is there has expired.
///
/// An expired icon that cannot be refreshed is kept and used: a site being
/// briefly unreachable is no reason to lose the icon it gave last month.
fn icon_for(client: &Client, directory: &Path, domain: &str) -> Result<Option<PathBuf>> {
    let icon = directory.join(format!("{}.icon", file_stem(domain)));
    let refused = directory.join(format!("{}.none", file_stem(domain)));
    let have = icon.is_file();
    if within(&refused, RETRY_AFTER) {
        return Ok(have.then_some(icon));
    }
    if have && within(&icon, KEEP) {
        return Ok(Some(icon));
    }
    let found = match KNOWN.iter().find(|(known, _)| *known == domain) {
        Some((_, url)) => fetch(client, url),
        None => CANDIDATES
            .iter()
            .find_map(|candidate| fetch(client, &format!("https://{domain}{candidate}"))),
    };
    let Some(image) = found else {
        fs::write(&refused, [])?;
        return Ok(have.then_some(icon));
    };
    fs::write(&icon, image)?;
    let _ = fs::remove_file(&refused);
    Ok(Some(icon))
}

/// The body at `url`, if it answered with an image.
///
/// A site that answers a missing favicon with its home page rather than a 404 is
/// common, so the bytes have to look like an image, not merely arrive.
fn fetch(client: &Client, url: &str) -> Option<Vec<u8>> {
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response.content_length().is_some_and(|size| size > LARGEST) {
        return None;
    }
    read_image(response)
}

fn read_image(reader: impl Read) -> Option<Vec<u8>> {
    // Content-Length is optional (and untrusted). Bound the read itself,
    // including chunked responses, before allocating the complete body.
    let mut body = Vec::new();
    reader.take(LARGEST + 1).read_to_end(&mut body).ok()?;
    (body.len() as u64 <= LARGEST && is_image(&body)).then_some(body)
}

/// Whether these bytes open like an image GPUI can draw.
fn is_image(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    head.starts_with(&[0x00, 0x00, 0x01, 0x00])        // ICO
        || head.starts_with(&[0x89, b'P', b'N', b'G']) // PNG
        || head.starts_with(&[0xFF, 0xD8, 0xFF])       // JPEG
        || head.starts_with(b"GIF8")                   // GIF
        || (head.starts_with(b"RIFF") && head.len() >= 12 && &head[8..12] == b"WEBP")
        || is_svg(head)
}

/// SVG, and not an HTML page that happens to draw one.
fn is_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

/// Whether a file exists and was written within the given age.
fn within(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|data| data.modified())
        .is_ok_and(|written| written.elapsed().is_ok_and(|since| since < age))
}

/// A domain as a file name. Domains are already restricted to letters, digits,
/// hyphens, and dots, but nothing here depends on the server honoring that.
fn file_stem(domain: &str) -> String {
    domain
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' | '.' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unbounded_response_is_stopped_at_the_size_limit() {
        let mut body = std::io::Cursor::new(vec![b'x'; LARGEST as usize * 2]);
        assert!(read_image(&mut body).is_none());
        assert_eq!(body.position(), LARGEST + 1);
        assert!(read_image(&b"\x89PNG\r\n\x1a\n"[..]).is_some());
    }

    #[test]
    fn only_bytes_that_open_like_an_image_are_kept() {
        assert!(is_image(&[0x00, 0x00, 0x01, 0x00, 1, 0]), "ICO");
        assert!(is_image(b"\x89PNG\r\n\x1a\n"), "PNG");
        assert!(is_image(b"\xff\xd8\xff\xe0"), "JPEG");
        assert!(is_image(b"GIF89a"), "GIF");
        assert!(is_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "WebP");
        assert!(
            is_image(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            "SVG"
        );
        assert!(
            is_image(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>"),
            "SVG behind an XML declaration"
        );

        // The case this guards against: a site answering a missing favicon with
        // its home page and a 200.
        assert!(!is_image(
            b"<!DOCTYPE html><html><body><svg/></body></html>"
        ));
        assert!(!is_image(b"<html><head><title>404</title></head></html>"));
        assert!(!is_image(b"{\"error\":\"not found\"}"));
        assert!(!is_image(b""));
        assert!(!is_image(b"RIFF"), "truncated RIFF header");
    }

    #[test]
    fn the_written_down_icons_are_addresses_a_domain_can_be_matched_against() {
        for (domain, url) in KNOWN {
            assert_eq!(domain, domain.to_lowercase(), "{domain} is matched as-is");
            assert!(!domain.contains('/'), "{domain} is a domain, not a URL");
            assert!(
                url.starts_with("https://"),
                "{url} is not fetched in the clear"
            );
        }
        let domains: Vec<&str> = KNOWN.iter().map(|(domain, _)| *domain).collect();
        let mut sorted = domains.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), domains.len(), "a domain is written down once");
    }

    #[test]
    fn a_domain_names_one_file_and_cannot_name_another_directory() {
        assert_eq!(file_stem("raulcarini.dev"), "raulcarini.dev");
        assert_eq!(file_stem("RailRadar24.com"), "railradar24.com");
        assert_eq!(file_stem("xn--to-6ja.example"), "xn--to-6ja.example");
        assert_eq!(file_stem("../../etc/passwd"), ".._.._etc_passwd");
        assert_eq!(file_stem("a b\\c:d"), "a_b_c_d");
    }
}

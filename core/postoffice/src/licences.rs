// SPDX-License-Identifier: MIT OR Apache-2.0
//! A copy of the FCC's amateur licence database, kept on the post office's own disk, for its
//! operators to look callsigns up (design D35).
//!
//! The FCC publishes every amateur licence in ULS each week as `l_amat.zip`, pipe-separated
//! records joined by a system identifier: `HD` (the licence: callsign, status, dates), `EN` (the
//! licensee: name and mailing address) and `AM` (the operator class, a club's trustee). The file
//! holds no email addresses: the FCC keeps them from public view. [`build`] reads the three and
//! writes one line per callsign, sorted, so [`Fcc::find`] answers by a binary search on disk:
//! no memory held, and no other site told whom the operators look up.
//!
//! What is found is evidence, not proof: anyone can read who holds a callsign.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Where the FCC publishes the weekly file of every amateur licence.
pub const FCC_URL: &str = "https://data.fcc.gov/download/pub/uls/complete/l_amat.zip";
/// How old the copy may be before the service fetches the next week's.
pub const FCC_EVERY_S: u64 = 7 * 86_400;

/// One callsign's licence, as the FCC's file has it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Licence {
    /// The callsign.
    pub call: String,
    /// `A` active, `E` expired, `C` cancelled, `T` terminated.
    pub status: String,
    /// `E` Extra, `A` Advanced, `G` General, `T` Technician, `N` Novice, `P` Technician Plus;
    /// empty for a club.
    pub class: String,
    /// When it was granted, `YYYY-MM-DD`.
    pub granted: String,
    /// When it expires.
    pub expires: String,
    /// When it was cancelled, if it was.
    pub cancelled: String,
    /// The licensee as one name: a person's in full, or a club's.
    pub name: String,
    /// A person's first name.
    pub first: String,
    /// A person's last name.
    pub last: String,
    /// The mailing address: street or PO box.
    pub street: String,
    /// The city.
    pub city: String,
    /// The state.
    pub state: String,
    /// The ZIP code.
    pub zip: String,
    /// A club's trustee.
    pub trustee: String,
}

const COLUMNS: usize = 14;

impl Licence {
    fn line(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ").trim().to_owned();
        [
            &self.call,
            &self.status,
            &self.class,
            &self.granted,
            &self.expires,
            &self.cancelled,
            &self.name,
            &self.first,
            &self.last,
            &self.street,
            &self.city,
            &self.state,
            &self.zip,
            &self.trustee,
        ]
        .iter()
        .map(|s| clean(s))
        .collect::<Vec<_>>()
        .join("\t")
    }

    fn parse(line: &str) -> Option<Licence> {
        let f: Vec<&str> = line.trim_end_matches(['\n', '\r']).split('\t').collect();
        if f.len() != COLUMNS {
            return None;
        }
        let s = |i: usize| f[i].to_owned();
        Some(Licence {
            call: s(0),
            status: s(1),
            class: s(2),
            granted: s(3),
            expires: s(4),
            cancelled: s(5),
            name: s(6),
            first: s(7),
            last: s(8),
            street: s(9),
            city: s(10),
            state: s(11),
            zip: s(12),
            trustee: s(13),
        })
    }

    /// The operator class in words.
    #[must_use]
    pub fn class_name(&self) -> &'static str {
        match self.class.as_str() {
            "E" => "Amateur Extra",
            "A" => "Advanced",
            "G" => "General",
            "T" => "Technician",
            "P" => "Technician Plus",
            "N" => "Novice",
            _ => "club",
        }
    }

    /// The status in words.
    #[must_use]
    pub fn status_name(&self) -> &'static str {
        match self.status.as_str() {
            "A" => "active",
            "E" => "expired",
            "C" => "cancelled",
            "T" => "terminated",
            _ => "of unknown status",
        }
    }

    /// The address to post a card to, one line each.
    #[must_use]
    pub fn mail_to(&self) -> Vec<String> {
        let mut lines = vec![self.name.clone()];
        if !self.street.is_empty() {
            lines.push(self.street.clone());
        }
        lines.push(
            format!("{}, {} {}", self.city, self.state, self.zip)
                .trim()
                .to_owned(),
        );
        lines
    }
}

/// `MM/DD/YYYY` as `YYYY-MM-DD`, so dates sort and read the same everywhere.
fn iso(date: &str) -> String {
    let p: Vec<&str> = date.split('/').collect();
    match p.as_slice() {
        [m, d, y] if y.len() == 4 => format!("{y}-{m}-{d}"),
        _ => String::new(),
    }
}

/// A US callsign by its prefix: AA–AL, K, N, W (ITU's allocation to the United States).
#[must_use]
pub fn us_callsign(call: &str) -> bool {
    let b = call.as_bytes();
    match b.first() {
        Some(b'K' | b'N' | b'W') => true,
        Some(b'A') => b.get(1).is_some_and(|c| (b'A'..=b'L').contains(c)),
        _ => false,
    }
}

/// The base of a callsign as on the air: no prefix, suffix or SSID.
#[must_use]
pub fn base_of(call: &str) -> String {
    let body = call.split('-').next().unwrap_or(call);
    body.split('/')
        .max_by_key(|p| (p.bytes().any(|b| b.is_ascii_digit()), p.len()))
        .unwrap_or(body)
        .to_uppercase()
}

/// Which licence to keep when a callsign has several (it was reissued): an active one, then the
/// latest granted.
fn better(a: &(String, String), b: &(String, String)) -> bool {
    let rank = |s: &str| u8::from(s == "A");
    (rank(&a.0), &a.1) > (rank(&b.0), &b.1)
}

/// Builds the sorted copy from the FCC's `HD`, `EN` and `AM` records, each read once. Returns how
/// many callsigns it holds.
///
/// # Errors
///
/// A record could not be read, or the copy could not be written.
pub fn build(
    hd: impl Read,
    en: impl Read,
    am: impl Read,
    out: &mut impl Write,
) -> Result<usize, String> {
    // the licence each callsign keeps, by its system identifier
    let mut best: HashMap<String, (String, (String, String))> = HashMap::new();
    let mut dates: HashMap<String, (String, String, String)> = HashMap::new();
    for line in BufReader::new(hd).split(b'\n') {
        let line = line.map_err(|e| format!("HD.dat: {e}"))?;
        let line = String::from_utf8_lossy(&line);
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 10 || f[0] != "HD" {
            continue;
        }
        let (usi, call, status) = (f[1].to_owned(), f[4].to_uppercase(), f[5].to_owned());
        let granted = iso(f[7]);
        let candidate = (status.clone(), granted.clone());
        let keep = best
            .get(&call)
            .is_none_or(|(_, held)| better(&candidate, held));
        if keep {
            best.insert(call, (usi.clone(), candidate));
        }
        dates.insert(usi, (granted, iso(f[8]), iso(f[9])));
    }
    let chosen: HashMap<String, String> = best
        .into_iter()
        .map(|(call, (usi, (status, _)))| (usi, format!("{call}\t{status}")))
        .collect();
    let mut class: HashMap<String, (String, String)> = HashMap::new();
    for line in BufReader::new(am).split(b'\n') {
        let line = line.map_err(|e| format!("AM.dat: {e}"))?;
        let line = String::from_utf8_lossy(&line);
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 9 || f[0] != "AM" || !chosen.contains_key(f[1]) {
            continue;
        }
        class.insert(f[1].to_owned(), (f[5].to_owned(), f[8].to_uppercase()));
    }
    let mut licences: Vec<Licence> = Vec::with_capacity(chosen.len());
    for line in BufReader::new(en).split(b'\n') {
        let line = line.map_err(|e| format!("EN.dat: {e}"))?;
        let line = String::from_utf8_lossy(&line);
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 20 || f[0] != "EN" {
            continue;
        }
        let Some(head) = chosen.get(f[1]) else {
            continue;
        };
        let (call, status) = head.split_once('\t').unwrap_or((head, ""));
        let (granted, expires, cancelled) = dates.get(f[1]).cloned().unwrap_or_default();
        let (op_class, trustee) = class.get(f[1]).cloned().unwrap_or_default();
        let name = [f[8], f[9], f[10], f[11]]
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| p.trim())
            .collect::<Vec<_>>()
            .join(" ");
        let street = if f[15].trim().is_empty() && !f[19].trim().is_empty() {
            format!("PO Box {}", f[19].trim())
        } else {
            f[15].to_owned()
        };
        licences.push(Licence {
            call: call.to_owned(),
            status: status.to_owned(),
            class: op_class,
            granted,
            expires,
            cancelled,
            name: if name.is_empty() {
                f[7].to_owned()
            } else {
                name
            },
            first: f[8].to_owned(),
            last: f[10].to_owned(),
            street,
            city: f[16].to_owned(),
            state: f[17].to_owned(),
            zip: f[18].chars().take(5).collect(),
            trustee,
        });
    }
    licences.sort_by(|a, b| a.call.cmp(&b.call));
    licences.dedup_by(|a, b| a.call == b.call);
    for l in &licences {
        writeln!(out, "{}", l.line()).map_err(|e| e.to_string())?;
    }
    Ok(licences.len())
}

/// Builds the copy from the FCC's zip file at `zip` into `out`. Returns how many callsigns.
///
/// # Errors
///
/// Not the FCC's file, or the copy could not be written.
pub fn build_from_zip(zip: &Path, out: &Path) -> Result<usize, String> {
    let open = || -> Result<zip::ZipArchive<File>, String> {
        let f = File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
        zip::ZipArchive::new(f).map_err(|e| format!("{}: not a zip file: {e}", zip.display()))
    };
    // each member read on its own handle, so the three stream side by side
    let (mut for_hd, mut for_en, mut for_am) = (open()?, open()?, open()?);
    let hd = for_hd
        .by_name("HD.dat")
        .map_err(|e| format!("HD.dat: {e}"))?;
    let en = for_en
        .by_name("EN.dat")
        .map_err(|e| format!("EN.dat: {e}"))?;
    let am = for_am
        .by_name("AM.dat")
        .map_err(|e| format!("AM.dat: {e}"))?;
    let part = out.with_extension("part");
    let mut writer =
        BufWriter::new(File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?);
    let count = build(hd, en, am, &mut writer)?;
    writer.flush().map_err(|e| e.to_string())?;
    drop(writer);
    std::fs::rename(&part, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let _ = std::fs::write(out.with_extension("count"), count.to_string());
    Ok(count)
}

/// The copy on disk, looked up by binary search.
pub struct Fcc {
    path: PathBuf,
}

impl Fcc {
    /// The copy at `path`, if there is one.
    #[must_use]
    pub fn open(path: &Path) -> Option<Fcc> {
        path.is_file().then(|| Fcc {
            path: path.to_owned(),
        })
    }

    /// The licence of `call` (its base callsign: no prefix, suffix or SSID), if the copy has it.
    ///
    /// # Errors
    ///
    /// The copy could not be read.
    pub fn find(&self, call: &str) -> Result<Option<Licence>, String> {
        let want = base_of(call);
        let mut file = File::open(&self.path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        // the first line that starts at or after `at`, and where it starts
        let mut line_at = |at: u64| -> Result<Option<(u64, String)>, String> {
            file.seek(SeekFrom::Start(at.saturating_sub(1)))
                .map_err(|e| e.to_string())?;
            let mut r = BufReader::new(&mut file);
            let mut start = at.saturating_sub(1);
            if at > 0 {
                let mut skip = Vec::new();
                start += r.read_until(b'\n', &mut skip).map_err(|e| e.to_string())? as u64;
            }
            let mut line = String::new();
            let n = r.read_line(&mut line).map_err(|e| e.to_string())?;
            Ok((n > 0).then_some((start, line)))
        };
        let (mut lo, mut hi) = (0u64, len);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let Some((start, line)) = line_at(mid)? else {
                hi = mid;
                continue;
            };
            let key = line.split('\t').next().unwrap_or("");
            match key.cmp(want.as_str()) {
                std::cmp::Ordering::Equal => return Ok(Licence::parse(&line)),
                std::cmp::Ordering::Less => lo = start + line.len() as u64,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        // the first line, which a search from the middle never starts at
        Ok(line_at(0)?
            .and_then(|(_, l)| Licence::parse(&l))
            .filter(|l| l.call == want))
    }

    /// How many callsigns it holds, and when it was made (Unix seconds).
    #[must_use]
    pub fn about(&self) -> (usize, u64) {
        let counted = std::fs::read_to_string(self.path.with_extension("count"))
            .ok()
            .and_then(|n| n.trim().parse().ok());
        let lines = counted.unwrap_or_else(|| {
            File::open(&self.path).map_or(0, |f| BufReader::new(f).split(b'\n').count())
        });
        let made = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        (lines, made)
    }
}

/// Fetches the FCC's weekly file into `dir` and builds the copy there as `fcc.tsv`, taking the
/// place of the old one only once it is whole. Returns how many callsigns.
///
/// # Errors
///
/// The FCC could not be reached, or the file was not what it should be.
pub fn update(dir: &Path, url: &str) -> Result<usize, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let zip = dir.join("l_amat.zip");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut body = response.body_mut().with_config().limit(1 << 31).reader();
    let mut file = BufWriter::new(File::create(&zip).map_err(|e| e.to_string())?);
    std::io::copy(&mut body, &mut file).map_err(|e| format!("{url}: {e}"))?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    let n = build_from_zip(&zip, &dir.join("fcc.tsv"));
    let _ = std::fs::remove_file(&zip);
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as the FCC writes it: `fields` at their positions, the rest empty.
    fn rec(kind: &str, width: usize, fields: &[(usize, &str)]) -> String {
        let mut f = vec![String::new(); width];
        f[0] = kind.to_owned();
        for (i, v) in fields {
            f[*i] = (*v).to_owned();
        }
        f.join("|") + "\n"
    }

    fn hd(usi: &str, call: &str, status: &str, dates: [&str; 3]) -> String {
        rec(
            "HD",
            59,
            &[
                (1, usi),
                (4, call),
                (5, status),
                (6, "HA"),
                (7, dates[0]),
                (8, dates[1]),
                (9, dates[2]),
            ],
        )
    }

    #[allow(clippy::too_many_arguments, reason = "an EN record's fields")]
    fn en(
        usi: &str,
        call: &str,
        entity: &str,
        names: [&str; 3],
        street: &str,
        place: [&str; 3],
        po_box: &str,
    ) -> String {
        rec(
            "EN",
            30,
            &[
                (1, usi),
                (4, call),
                (5, "L"),
                (7, entity),
                (8, names[0]),
                (9, names[1]),
                (10, names[2]),
                (15, street),
                (16, place[0]),
                (17, place[1]),
                (18, place[2]),
                (19, po_box),
            ],
        )
    }

    fn am(usi: &str, call: &str, class: &str, trustee: &str) -> String {
        rec("AM", 18, &[(1, usi), (4, call), (5, class), (8, trustee)])
    }

    fn records() -> (String, String, String) {
        let hd = [
            hd("10", "W1AW", "A", ["12/08/2020", "02/26/2031", ""]),
            hd("20", "N5CALL", "E", ["01/02/2010", "01/02/2020", ""]),
            hd("21", "N5CALL", "A", ["03/04/2024", "03/04/2034", ""]),
            hd(
                "30",
                "N6CALL",
                "C",
                ["05/06/2015", "05/06/2025", "07/08/2019"],
            ),
        ]
        .concat();
        let en = [
            en(
                "10",
                "W1AW",
                "ARRL HQ OPERATORS CLUB",
                ["", "", ""],
                "225 MAIN ST",
                ["NEWINGTON", "CT", "06111"],
                "",
            ),
            en(
                "20",
                "N5CALL",
                "Old Holder",
                ["OLD", "", "HOLDER"],
                "1 ROAD",
                ["SOMEWHERE", "TX", "75001"],
                "",
            ),
            en(
                "21",
                "N5CALL",
                "Jose Perez",
                ["JOSE", "A", "PEREZ"],
                "12 ELM ST",
                ["ATLANTA", "GA", "30303-1234"],
                "",
            ),
            en(
                "30",
                "N6CALL",
                "Lena Ortiz",
                ["LENA", "", "ORTIZ"],
                "",
                ["MACON", "GA", "31201"],
                "77",
            ),
        ]
        .concat();
        let am = [
            am("10", "W1AW", "", "AF5ID"),
            am("20", "N5CALL", "T", ""),
            am("21", "N5CALL", "G", ""),
            am("30", "N6CALL", "E", ""),
        ]
        .concat();
        (hd, en, am)
    }

    fn copy() -> (Fcc, PathBuf) {
        let dir = std::env::temp_dir().join(format!("radiopost-fcc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join("fcc.tsv");
        let mut out = Vec::new();
        let (hd, en, am) = records();
        let n = build(hd.as_bytes(), en.as_bytes(), am.as_bytes(), &mut out).expect("built");
        assert_eq!(n, 3);
        std::fs::write(&path, out).expect("written");
        (Fcc::open(&path).expect("there"), dir)
    }

    #[test]
    fn a_reissued_callsign_keeps_its_active_licence() {
        let (fcc, dir) = copy();
        let l = fcc.find("n5call-7").expect("read").expect("found");
        assert_eq!(
            (l.status.as_str(), l.class.as_str(), l.name.as_str()),
            ("A", "G", "JOSE A PEREZ")
        );
        assert_eq!(
            (l.granted.as_str(), l.expires.as_str()),
            ("2024-03-04", "2034-03-04")
        );
        assert_eq!(
            l.mail_to(),
            ["JOSE A PEREZ", "12 ELM ST", "ATLANTA, GA 30303"]
        );
        let club = fcc.find("W1AW").expect("read").expect("found");
        assert_eq!(
            (club.class_name(), club.trustee.as_str()),
            ("club", "AF5ID")
        );
        let gone = fcc.find("VE3/N6CALL/P").expect("read").expect("found");
        assert_eq!(
            (gone.status_name(), gone.cancelled.as_str()),
            ("cancelled", "2019-07-08")
        );
        assert_eq!(gone.street, "PO Box 77");
        assert_eq!(fcc.find("N0CALL").expect("read"), None);
        assert_eq!(fcc.find("AAAA").expect("read"), None);
        assert_eq!(fcc.find("ZZZZ").expect("read"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn every_callsign_is_found_by_the_binary_search() {
        let dir = std::env::temp_dir().join(format!("radiopost-fcc-many-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let (mut hd, mut en, mut am) = (String::new(), String::new(), String::new());
        for i in 0..2000 {
            let (usi, call) = (i.to_string(), format!("K{i}AB"));
            hd.push_str(&super::tests::hd(
                &usi,
                &call,
                "A",
                ["01/01/2025", "01/01/2035", ""],
            ));
            en.push_str(&super::tests::en(
                &usi,
                &call,
                "",
                ["N", "", &usi],
                "ST",
                ["CITY", "GA", "30000"],
                "",
            ));
            am.push_str(&super::tests::am(&usi, &call, "G", ""));
        }
        let mut out = Vec::new();
        build(hd.as_bytes(), en.as_bytes(), am.as_bytes(), &mut out).expect("built");
        std::fs::write(dir.join("fcc.tsv"), out).expect("written");
        let fcc = Fcc::open(&dir.join("fcc.tsv")).expect("there");
        for i in 0..2000 {
            let call = format!("K{i}AB");
            assert_eq!(fcc.find(&call).expect("read").map(|l| l.call), Some(call));
        }
        assert_eq!(fcc.about().0, 2000);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn us_callsigns_by_prefix() {
        for c in ["KK4ODA", "N5CALL", "W1AW", "AA1A", "AL7X"] {
            assert!(us_callsign(c), "{c}");
        }
        for c in ["VE3ABC", "G4ABC", "AM1A", "EA5HVK"] {
            assert!(!us_callsign(c), "{c}");
        }
        assert_eq!(base_of("VE3/KK4ODA/P-2"), "KK4ODA");
        assert_eq!(base_of("w4/g4abc"), "G4ABC");
    }
}

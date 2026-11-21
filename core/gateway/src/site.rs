// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the gateway's antennas are, and how it transmits on each channel, as its operator types
//! them (RP-5 §4.4 `site` and a channel's `station`): checked against the ranges a reader keeps,
//! so a value a caller's program would ignore is refused here, with why, rather than registered.
//! A position keeps the precision the operator typed: two decimal places are about a kilometre,
//! four about 11 m.

use serde_json::{Value, json};

/// A number the operator typed, or `None` when the field is empty.
fn typed(what: &str, text: &str) -> Result<Option<f64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .map(Some)
        .ok_or_else(|| format!("{what}: {t} is not a number"))
}

fn within(
    what: &str,
    x: Option<f64>,
    ok: impl Fn(f64) -> bool,
    range: &str,
) -> Result<Option<f64>, String> {
    match x {
        Some(x) if !ok(x) => Err(format!("{what}: {x} is outside {range}")),
        _ => Ok(x),
    }
}

/// The record's `site` from the latitude, longitude and antenna height typed: `None` when all
/// three are empty.
///
/// # Errors
///
/// One is not a number or out of its range, or a height is given with no position.
pub fn site(lat: &str, lon: &str, agl: &str) -> Result<Option<Value>, String> {
    let lat = within(
        "latitude",
        typed("latitude", lat)?,
        |x| (-90.0..=90.0).contains(&x),
        "−90 to 90 degrees, north positive",
    )?;
    let lon = within(
        "longitude",
        typed("longitude", lon)?,
        |x| (-180.0..=180.0).contains(&x),
        "−180 to 180 degrees, east positive",
    )?;
    let agl = within(
        "antenna height",
        typed("antenna height", agl)?,
        |x| (0.0..=1000.0).contains(&x),
        "0 to 1000 m",
    )?;
    match (lat, lon) {
        (Some(lat), Some(lon)) => {
            let mut s = json!({"lat": lat, "lon": lon});
            if let Some(h) = agl {
                s["agl_m"] = json!(h);
            }
            Ok(Some(s))
        }
        (None, None) if agl.is_none() => Ok(None),
        _ => Err("the site needs both its latitude and its longitude".into()),
    }
}

/// A channel's `station` from what was typed: `None` when every field is empty.
///
/// # Errors
///
/// One is not a number, out of its range, or an antenna kind RP-0 §9.6 does not name.
pub fn station(
    power: &str,
    antenna: &str,
    agl: &str,
    gain: &str,
    azimuth: &str,
) -> Result<Option<Value>, String> {
    let mut s = json!({});
    if let Some(p) = within("power", typed("power", power)?, |x| x > 0.0, "above 0 W")? {
        s["power_w"] = json!(p);
    }
    let antenna = antenna.trim();
    if !antenna.is_empty() {
        if !radiopost::gateway_list::ANTENNAS.contains(&antenna) {
            return Err(format!(
                "antenna: {antenna} is none of {}",
                radiopost::gateway_list::ANTENNAS.join(", ")
            ));
        }
        s["antenna"] = json!(antenna);
    }
    if let Some(h) = within(
        "antenna height",
        typed("antenna height", agl)?,
        |x| (0.0..=1000.0).contains(&x),
        "0 to 1000 m",
    )? {
        s["agl_m"] = json!(h);
    }
    if let Some(g) = within(
        "gain",
        typed("gain", gain)?,
        |x| (-30.0..=60.0).contains(&x),
        "−30 to 60 dBi",
    )? {
        s["gain_dbi"] = json!(g);
    }
    if let Some(z) = within(
        "azimuth",
        typed("azimuth", azimuth)?,
        |x| (0.0..360.0).contains(&x),
        "0 to below 360 degrees",
    )? {
        s["azimuth_deg"] = json!(z);
    }
    Ok((s.as_object().is_some_and(|o| !o.is_empty())).then_some(s))
}

/// The Maidenhead square (4 characters) a position is in.
#[must_use]
pub fn square(lat: f64, lon: f64) -> String {
    let lon = (lon + 180.0).clamp(0.0, 359.999_999);
    let lat = (lat + 90.0).clamp(0.0, 179.999_999);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped above: a field 0 to 17, a square's digit 0 to 9"
    )]
    let step = |n: f64| n.floor() as u8;
    let letter = |n: f64| char::from(b'A' + step(n));
    let digit = |n: f64| char::from(b'0' + step(n));
    [
        letter(lon / 20.0),
        letter(lat / 10.0),
        digit((lon % 20.0) / 2.0),
        digit(lat % 10.0),
    ]
    .iter()
    .collect()
}

/// Refuses a `grid` whose square does not hold the `site` (RP-5 §4.4: it SHOULD), naming the one
/// that does.
///
/// # Errors
///
/// The grid's first four characters are another square's.
pub fn agrees(grid: &str, site: &Value) -> Result<(), String> {
    let (Some(lat), Some(lon)) = (site["lat"].as_f64(), site["lon"].as_f64()) else {
        return Ok(());
    };
    let here = square(lat, lon);
    let g = grid.trim();
    if g.len() >= 4 && !g[..4].eq_ignore_ascii_case(&here) {
        return Err(format!(
            "the grid {g} does not hold the site, which is in {here}"
        ));
    }
    Ok(())
}

/// The command line's `--site LAT,LON[,AGL]`.
///
/// # Errors
///
/// As [`site`], or not two or three numbers.
pub fn site_flag(text: &str) -> Result<Value, String> {
    let p: Vec<&str> = text.split(',').collect();
    if !(2..=3).contains(&p.len()) {
        return Err(format!(
            "{text}: the site is LAT,LON or LAT,LON,AGL (32.8407,-83.6324,18)"
        ));
    }
    site(p[0], p[1], p.get(2).copied().unwrap_or(""))?.ok_or_else(|| "an empty site".into())
}

/// The command line's `--station HZ,POWER,ANTENNA,AGL,GAIN[,AZIMUTH]`: the channel's dial and its
/// station, any field but the dial empty.
///
/// # Errors
///
/// As [`station`], or no dial.
pub fn station_flag(text: &str) -> Result<(u64, Value), String> {
    let p: Vec<&str> = text.split(',').collect();
    let hz = p.first().and_then(|x| x.trim().parse::<u64>().ok()).ok_or_else(|| {
        format!("{text}: a station is HZ,POWER,ANTENNA,AGL,GAIN[,AZIMUTH] (145050000,50,j-pole,18,3)")
    })?;
    let f = |i: usize| p.get(i).copied().unwrap_or("");
    let s = station(f(1), f(2), f(3), f(4), f(5))?
        .ok_or_else(|| format!("{text}: an empty station"))?;
    Ok((hz, s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_keeps_the_precision_typed() {
        assert_eq!(site("", "", "").unwrap(), None);
        let s = site("32.8407", "-83.6324", "18").unwrap().unwrap();
        assert_eq!(s, json!({"lat": 32.8407, "lon": -83.6324, "agl_m": 18.0}));
        // written as typed: four places stay four, two stay two
        assert!(
            s.to_string().contains(r#""lat":32.8407,"lon":-83.6324"#),
            "{s}"
        );
        let s = site(" 32.84 ", "-83.63", "").unwrap().unwrap();
        assert_eq!(s.to_string(), r#"{"lat":32.84,"lon":-83.63}"#);
        // what a caller's program would ignore is refused here, saying why
        assert!(site("91", "-83.63", "").unwrap_err().contains("latitude"));
        assert!(site("32.84", "-183", "").unwrap_err().contains("longitude"));
        assert!(
            site("32.84", "-83.63", "-2")
                .unwrap_err()
                .contains("antenna height")
        );
        assert!(site("32.84", "", "").unwrap_err().contains("both"));
        assert!(site("", "", "18").unwrap_err().contains("both"));
        assert!(
            site("north", "-83.63", "")
                .unwrap_err()
                .contains("not a number")
        );
        // and what it keeps, a reader keeps
        let s = site("-90", "180", "1000").unwrap().unwrap();
        let read = radiopost::gateway_list::site(&json!({"site": s})).expect("read");
        assert_eq!(
            (read.lat, read.lon, read.agl_m),
            (-90.0, 180.0, Some(1000.0))
        );
    }

    #[test]
    fn a_station_keeps_each_member_given() {
        assert_eq!(station("", "", "", "", "").unwrap(), None);
        let s = station("50", "j-pole", "18", "3", "").unwrap().unwrap();
        assert_eq!(
            s,
            json!({"power_w": 50.0, "antenna": "j-pole", "agl_m": 18.0, "gain_dbi": 3.0})
        );
        let read = radiopost::gateway_list::station(&json!({"station": s}));
        assert_eq!(
            (read.power_w, read.antenna.as_deref(), read.azimuth_deg),
            (Some(50.0), Some("j-pole"), None)
        );
        assert!(station("0", "", "", "", "").unwrap_err().contains("power"));
        assert!(
            station("", "rhombic", "", "", "")
                .unwrap_err()
                .contains("none of vertical")
        );
        assert!(station("", "", "", "61", "").unwrap_err().contains("gain"));
        assert!(
            station("", "", "", "", "360")
                .unwrap_err()
                .contains("azimuth")
        );
        assert_eq!(
            station("", "yagi", "", "", "270").unwrap().unwrap(),
            json!({"antenna": "yagi", "azimuth_deg": 270.0})
        );
    }

    #[test]
    fn a_grid_must_hold_the_site() {
        // Macon, Georgia: 32.84 N, 83.63 W is in EM82
        assert_eq!(square(32.8407, -83.6324), "EM82");
        assert_eq!(square(33.75, -84.39), "EM73");
        assert_eq!(square(-33.86, 151.21), "QF56");
        let s = site("32.8407", "-83.6324", "").unwrap().unwrap();
        assert!(agrees("EM82", &s).is_ok());
        assert!(agrees("em82xu", &s).is_ok());
        assert!(agrees("", &s).is_ok());
        assert_eq!(
            agrees("EM73", &s).unwrap_err(),
            "the grid EM73 does not hold the site, which is in EM82"
        );
    }

    #[test]
    fn the_command_line_names_them_too() {
        assert_eq!(
            site_flag("32.8407,-83.6324,18").unwrap(),
            json!({"lat": 32.8407, "lon": -83.6324, "agl_m": 18.0})
        );
        assert!(site_flag("32.84").is_err());
        let (hz, s) = station_flag("145050000,50,j-pole,18,3").unwrap();
        assert_eq!(
            (hz, s),
            (
                145_050_000,
                json!({"power_w": 50.0, "antenna": "j-pole", "agl_m": 18.0, "gain_dbi": 3.0})
            )
        );
        let (_, s) = station_flag("145050000,,yagi,,,90").unwrap();
        assert_eq!(s, json!({"antenna": "yagi", "azimuth_deg": 90.0}));
        assert!(station_flag("145050000").is_err());
        assert!(station_flag("fm,50").is_err());
    }
}

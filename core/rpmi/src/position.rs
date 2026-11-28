// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a beacon says its station is (RP-4 §4.4): a latitude and longitude in degrees and how
//! precisely they go, the whole of it or the 4- or 6-character Maidenhead square. A modem sends as
//! much as the precision says, and a station that hears a square hears its centre and its locator.
//!
//! A square's centre is worked in whole twenty-fourths and forty-eighths of a degree and divided
//! once, as the reference does, so that every program reading one square gets the same two
//! numbers to the last bit: the vectors compare them as JSON values.

use serde_json::{Map, Value, json};

/// How precisely a beacon says where its station is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    /// The 4-character square: 2° of longitude by 1° of latitude.
    Grid4,
    /// The 6-character square: 5′ of longitude by 2.5′ of latitude.
    Grid6,
    /// The position itself.
    Exact,
}

impl Precision {
    /// Its name on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Grid4 => "grid4",
            Precision::Grid6 => "grid6",
            Precision::Exact => "exact",
        }
    }

    /// The precision a name gives, if it is one.
    #[must_use]
    pub fn named(name: &str) -> Option<Precision> {
        match name {
            "grid4" => Some(Precision::Grid4),
            "grid6" => Some(Precision::Grid6),
            "exact" => Some(Precision::Exact),
            _ => None,
        }
    }
}

/// A position as RP-4 §4.4 carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    /// Degrees north, −90 to 90.
    pub lat: f64,
    /// Degrees east, −180 to 180.
    pub lon: f64,
    /// How precisely it goes, or went.
    pub precision: Precision,
    /// The locator, for a square heard.
    pub grid: Option<String>,
}

impl Position {
    /// A position from its JSON object; `None` when a member is missing, of the wrong type or out
    /// of range.
    #[must_use]
    pub fn from_json(v: &Value) -> Option<Position> {
        let o = v.as_object()?;
        let number = |k: &str| o.get(k).filter(|n| n.is_number()).and_then(Value::as_f64);
        let lat = number("lat").filter(|l| (-90.0..=90.0).contains(l))?;
        let lon = number("lon").filter(|l| (-180.0..=180.0).contains(l))?;
        let precision = Precision::named(o.get("precision")?.as_str()?)?;
        let grid = o.get("grid").and_then(Value::as_str).map(str::to_owned);
        Some(Position {
            lat,
            lon,
            precision,
            grid,
        })
    }

    /// Its JSON object.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("lat".into(), json!(self.lat));
        m.insert("lon".into(), json!(self.lon));
        m.insert("precision".into(), self.precision.as_str().into());
        if let Some(grid) = &self.grid {
            m.insert("grid".into(), grid.clone().into());
        }
        Value::Object(m)
    }

    /// What a station hears of this position, sent at its precision: the latitude and longitude
    /// themselves for `exact`; the square's centre and its locator for a grid.
    #[must_use]
    pub fn heard(&self) -> Position {
        let chars = match self.precision {
            Precision::Exact => {
                return Position {
                    grid: None,
                    ..self.clone()
                };
            }
            Precision::Grid4 => 4,
            Precision::Grid6 => 6,
        };
        let grid = locator(self.lat, self.lon, chars);
        let (lat, lon) = square_centre(&grid).unwrap_or((self.lat, self.lon));
        Position {
            lat,
            lon,
            precision: self.precision,
            grid: Some(grid),
        }
    }
}

/// Whether a JSON value is a position a host may send (RP-4 §4.4).
#[must_use]
pub fn is_position(v: &Value) -> bool {
    Position::from_json(v).is_some()
}

/// The 4- or 6-character Maidenhead locator of the square that holds a position.
#[must_use]
pub fn locator(lat: f64, lon: f64, chars: usize) -> String {
    let x = lon + 180.0;
    let y = lat + 90.0;
    // whole cells, clamped so that the north pole and the antimeridian stay in the last ones
    let cell = |v: f64, most: u8| -> u8 {
        if v <= 0.0 {
            0
        } else {
            let whole = v.floor();
            if whole >= f64::from(most) {
                most
            } else {
                // whole is in 0..most, so the cast keeps it
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    whole as u8
                }
            }
        }
    };
    let field_x = cell(x / 20.0, 17);
    let field_y = cell(y / 10.0, 17);
    let square_x = cell(x.rem_euclid(20.0) / 2.0, 9);
    let square_y = cell(y.rem_euclid(10.0), 9);
    let mut out = String::new();
    out.push(char::from(b'A' + field_x));
    out.push(char::from(b'A' + field_y));
    out.push(char::from(b'0' + square_x));
    out.push(char::from(b'0' + square_y));
    if chars == 6 {
        let sub_x = cell(x.rem_euclid(2.0) * 12.0, 23);
        let sub_y = cell(y.rem_euclid(1.0) * 24.0, 23);
        out.push(char::from(b'a' + sub_x));
        out.push(char::from(b'a' + sub_y));
    }
    out
}

/// The centre of a 4- or 6-character Maidenhead square, as (lat, lon); `None` for a locator that
/// is not one.
#[must_use]
pub fn square_centre(grid: &str) -> Option<(f64, f64)> {
    let b = grid.as_bytes();
    if b.len() != 4 && b.len() != 6 {
        return None;
    }
    let up = |c: u8, most: u8| {
        c.to_ascii_uppercase()
            .checked_sub(b'A')
            .filter(|&v| v <= most)
    };
    let digit = |c: u8| c.checked_sub(b'0').filter(|&v| v <= 9);
    let field_x = i32::from(up(b[0], 17)?);
    let field_y = i32::from(up(b[1], 17)?);
    let square_x = i32::from(digit(b[2])?);
    let square_y = i32::from(digit(b[3])?);
    if b.len() == 4 {
        let lat = f64::from(-90 + field_y * 10 + square_y) + 0.5;
        let lon = f64::from(-180 + field_x * 20 + square_x * 2 + 1);
        return Some((lat, lon));
    }
    let sub_x = i32::from(up(b[4], 23)?);
    let sub_y = i32::from(up(b[5], 23)?);
    let lon_24ths = (field_x * 20 + square_x * 2) * 24 + sub_x * 2 + 1;
    let lat_48ths = (field_y * 10 + square_y) * 48 + sub_y * 2 + 1;
    Some((
        -90.0 + f64::from(lat_48ths) / 48.0,
        -180.0 + f64::from(lon_24ths) / 24.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::{Position, Precision, locator, square_centre};
    use serde_json::json;

    #[test]
    fn squares_are_found_and_centred() {
        assert_eq!(locator(34.4621, -83.7514, 6), "EM84cl");
        assert_eq!(locator(33.78123, -84.38765, 4), "EM73");
        let (lat, lon) = square_centre("EM84cl").expect("a square");
        assert!((lat - (34.0 + 23.0 / 48.0)).abs() < 1e-12, "{lat}");
        assert!((lon + (83.0 + 19.0 / 24.0)).abs() < 1e-12, "{lon}");
        assert_eq!(square_centre("EM73"), Some((33.5, -85.0)));
        assert_eq!(square_centre("em84CL"), square_centre("EM84cl"));
        assert_eq!(square_centre("EM8"), None);
        assert_eq!(square_centre("ZZ99"), None);
        // the corners stay in the grid
        assert_eq!(locator(89.999, 179.999, 6), "RR99xx");
        assert_eq!(locator(-90.0, -180.0, 6), "AA00aa");
    }

    #[test]
    fn a_position_reads_in_range_and_is_heard_at_its_precision() {
        let sent =
            Position::from_json(&json!({"lat": 34.4621, "lon": -83.7514, "precision": "grid6"}))
                .expect("a position");
        let heard = sent.heard();
        assert_eq!(heard.grid.as_deref(), Some("EM84cl"));
        assert_eq!(heard.precision, Precision::Grid6);
        let exact = Position::from_json(&json!({"lat": 1, "lon": 2, "precision": "exact"}))
            .expect("integers are numbers");
        assert_eq!(exact.heard(), exact);
        for bad in [
            json!({"lat": 91, "lon": 0, "precision": "exact"}),
            json!({"lat": 0, "lon": 181, "precision": "exact"}),
            json!({"lat": 0, "lon": 0}),
            json!({"lat": "0", "lon": 0, "precision": "exact"}),
            json!({"lat": 0, "lon": 0, "precision": "grid8"}),
            json!([0, 0]),
        ] {
            assert_eq!(Position::from_json(&bad), None, "{bad}");
        }
    }
}

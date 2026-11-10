// SPDX-License-Identifier: MIT OR Apache-2.0
//! Maidenhead grid squares (`EM73`, `EM73tt`), as amateurs give where they are: the centre of a
//! square, and the distance between two, to rank gateways by how near they are (design §5.1).

/// The centre of a grid square of four or six characters, as (latitude, longitude) in degrees.
#[must_use]
pub fn centre(grid: &str) -> Option<(f64, f64)> {
    let g: Vec<char> = grid.trim().chars().collect();
    if g.len() != 4 && g.len() != 6 {
        return None;
    }
    let field = |c: char| {
        ('A'..='R')
            .contains(&c.to_ascii_uppercase())
            .then(|| f64::from(c.to_ascii_uppercase() as u8 - b'A'))
    };
    let square = |c: char| c.to_digit(10).map(f64::from);
    let sub = |c: char| {
        ('a'..='x')
            .contains(&c.to_ascii_lowercase())
            .then(|| f64::from(c.to_ascii_lowercase() as u8 - b'a'))
    };
    let mut lon = field(g[0])? * 20.0 - 180.0 + square(g[2])? * 2.0;
    let mut lat = field(g[1])? * 10.0 - 90.0 + square(g[3])?;
    if g.len() == 6 {
        lon += sub(g[4])? * 2.0 / 24.0 + 1.0 / 24.0;
        lat += sub(g[5])? / 24.0 + 0.5 / 24.0;
    } else {
        lon += 1.0;
        lat += 0.5;
    }
    Some((lat, lon))
}

/// The distance between the centres of two grid squares, in kilometres, on a sphere.
#[must_use]
pub fn km(a: &str, b: &str) -> Option<f64> {
    let ((lat1, lon1), (lat2, lon2)) = (centre(a)?, centre(b)?);
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    Some(2.0 * 6371.0 * h.sqrt().asin())
}

#[cfg(test)]
mod tests {
    use super::{centre, km};

    #[test]
    fn centres() {
        // EM73: 86–84° W, 33–34° N (Atlanta, at 84.4° W, is in it)
        assert_eq!(centre("EM73"), Some((33.5, -85.0)));
        assert_eq!(centre("em73"), Some((33.5, -85.0)));
        // JO01: 0–2° E, 51–52° N
        assert_eq!(centre("JO01"), Some((51.5, 1.0)));
        let (lat, lon) = centre("EM73tt").expect("a centre");
        assert!(
            (lat - (33.0 + 19.5 / 24.0)).abs() < 1e-9 && (lon - (-86.0 + 39.0 / 24.0)).abs() < 1e-9
        );
        for bad in ["", "EM7", "EM7X", "ZZ00", "EM73zz", "EM73t"] {
            assert_eq!(centre(bad), None, "{bad}");
        }
    }

    #[test]
    fn distances() {
        assert_eq!(km("EM73", "EM73"), Some(0.0));
        // Atlanta's square to London's: about 6 800 km
        let d = km("EM73", "IO91").expect("a distance");
        assert!((6_700.0..6_900.0).contains(&d), "{d}");
        // one square north is a degree of latitude: about 111 km
        let d = km("EM73", "EM74").expect("a distance");
        assert!((110.0..112.5).contains(&d), "{d}");
        assert_eq!(km("EM73", "nowhere"), None);
    }
}

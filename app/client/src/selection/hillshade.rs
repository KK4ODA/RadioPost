// SPDX-License-Identifier: MIT OR Apache-2.0
//! The terrain map's background (the gateway selection design, §16.2): the ground held on this
//! computer drawn as hillshade, lit from the north-west as maps are, the sea pale blue, and nothing
//! where no tile is held. No map tiles are fetched: it is drawn from the elevation data the paths
//! are worked out with, so it is there wherever they are.

use std::io::Write;

use radioprop_geo::LatLon;

/// The widest picture drawn, pixels.
pub const MAX_PIXELS: u32 = 1024;
/// The most ground a picture may cover, degrees a side.
pub const MAX_SPAN_DEG: f64 = 4.0;
/// The light: from the north-west (315°), 45° up.
const SUN_AZIMUTH_DEG: f64 = 315.0;
const SUN_ALTITUDE_DEG: f64 = 45.0;
/// Relief is drawn twice as steep as it is, so gentle country still shows at a few hundred metres
/// a pixel.
const Z_FACTOR: f64 = 2.0;
/// Metres in a degree of latitude, and of longitude at the equator.
const M_PER_DEG_LAT: f64 = 110_540.0;
const M_PER_DEG_LON: f64 = 111_320.0;

/// The ground a picture covers, degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    /// The southern edge, degrees north.
    pub south: f64,
    /// The western edge, degrees east.
    pub west: f64,
    /// The northern edge, degrees north.
    pub north: f64,
    /// The eastern edge, degrees east.
    pub east: f64,
}

impl Area {
    /// The area, if it is one a picture can cover.
    ///
    /// # Errors
    ///
    /// A side is not a latitude or longitude, the area is empty, or it is wider than
    /// [`MAX_SPAN_DEG`].
    pub fn new(south: f64, west: f64, north: f64, east: f64) -> Result<Area, String> {
        let lat_ok = |x: f64| (-90.0..=90.0).contains(&x);
        let lon_ok = |x: f64| (-180.0..=180.0).contains(&x);
        if !(lat_ok(south) && lat_ok(north) && lon_ok(west) && lon_ok(east)) {
            return Err("the area: latitudes −90 to 90, longitudes −180 to 180".into());
        }
        if south >= north || west >= east {
            return Err("the area: south below north, west before east".into());
        }
        if north - south > MAX_SPAN_DEG || east - west > MAX_SPAN_DEG {
            return Err(format!("the area: at most {MAX_SPAN_DEG}° a side"));
        }
        Ok(Area {
            south,
            west,
            north,
            east,
        })
    }

    /// The picture's height for `width` pixels across, so that a pixel is square on the ground.
    #[must_use]
    pub fn height_for(&self, width: u32) -> u32 {
        let mid = f64::midpoint(self.south, self.north).to_radians().cos();
        let ratio = (self.north - self.south) * M_PER_DEG_LAT
            / ((self.east - self.west) * M_PER_DEG_LON * mid).max(1e-9);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 1 to MAX_PIXELS"
        )]
        let h = (f64::from(width) * ratio)
            .round()
            .clamp(1.0, f64::from(MAX_PIXELS)) as u32;
        h
    }
}

/// The picture: `width` pixels across `area`, RGBA, a row at a time from the north. `height` gives
/// the ground's height in metres at a point, or `None` where no tile is held; 0 m and below, on
/// level ground, is the sea.
#[must_use]
pub fn draw(area: Area, width: u32, height: &dyn Fn(LatLon) -> Option<f64>) -> (u32, u32, Vec<u8>) {
    let w = width.clamp(1, MAX_PIXELS);
    let h = area.height_for(w);
    let (wu, hu) = (w as usize, h as usize);
    let dlat = (area.north - area.south) / f64::from(h);
    let dlon = (area.east - area.west) / f64::from(w);
    let mut z: Vec<Option<f64>> = Vec::with_capacity(wu * hu);
    for r in 0..h {
        let lat = area.north - (f64::from(r) + 0.5) * dlat;
        for c in 0..w {
            let lon = area.west + (f64::from(c) + 0.5) * dlon;
            z.push(height(LatLon { lat, lon }));
        }
    }
    let (zen, az) = (
        (90.0 - SUN_ALTITUDE_DEG).to_radians(),
        SUN_AZIMUTH_DEG.to_radians(),
    );
    let mut out = Vec::with_capacity(wu * hu * 4);
    for r in 0..h {
        let lat = area.north - (f64::from(r) + 0.5) * dlat;
        let dx = dlon * M_PER_DEG_LON * lat.to_radians().cos().max(1e-6);
        let dy = dlat * M_PER_DEG_LAT;
        for c in 0..w {
            let Some(here) = z[(r * w + c) as usize] else {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            };
            // Horn's method on the 3 × 3 around the pixel, an edge or a missing height read as here
            let at = |dr: i32, dc: i32| {
                match (r.checked_add_signed(dr), c.checked_add_signed(dc)) {
                    (Some(rr), Some(cc)) if rr < h && cc < w => z[(rr * w + cc) as usize],
                    _ => None,
                }
                .unwrap_or(here)
            };
            let rise_east = ((at(-1, 1) + 2.0 * at(0, 1) + at(1, 1))
                - (at(-1, -1) + 2.0 * at(0, -1) + at(1, -1)))
                / (8.0 * dx);
            // rows run south, so the ground rises to the north when the rows above are higher
            let rise_north = ((at(-1, -1) + 2.0 * at(-1, 0) + at(-1, 1))
                - (at(1, -1) + 2.0 * at(1, 0) + at(1, 1)))
                / (8.0 * dy);
            let (gx, gy) = (Z_FACTOR * rise_east, Z_FACTOR * rise_north);
            let slope = gx.hypot(gy).atan();
            // the way the ground faces (downhill), clockwise from north
            let aspect = (-gx).atan2(-gy);
            let light =
                (zen.cos() * slope.cos() + zen.sin() * slope.sin() * (az - aspect).cos()).max(0.0);
            if here <= 0.0 && slope < 0.002 {
                out.extend_from_slice(&[178, 205, 228, 255]);
                continue;
            }
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "60 to 255"
            )]
            let v = (60.0 + 195.0 * light).round().clamp(0.0, 255.0) as u8;
            out.extend_from_slice(&[v, v, v.saturating_sub(6), 255]);
        }
    }
    (w, h, out)
}

/// An RGBA picture as a PNG file (ISO/IEC 15948): one IDAT, every row unfiltered.
#[must_use]
pub fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: [u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(&kind);
        crc.update(data);
        out.extend_from_slice(&kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // 8 bits a sample, colour type 6 (RGBA), deflate, adaptive filtering, no interlace
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, *b"IHDR", &header);
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((row + 1) * height as usize);
    for line in rgba.chunks(row) {
        raw.push(0);
        raw.extend_from_slice(line);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = z.write_all(&raw);
    chunk(&mut out, *b"IDAT", &z.finish().unwrap_or_default());
    chunk(&mut out, *b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn area() -> Area {
        Area::new(33.0, -85.0, 34.0, -84.0).expect("an area")
    }

    #[test]
    fn an_area_is_checked_and_its_pixels_are_square_on_the_ground() {
        assert!(Area::new(34.0, -85.0, 33.0, -84.0).is_err(), "upside down");
        assert!(Area::new(33.0, -85.0, 38.0, -84.0).is_err(), "too tall");
        assert!(Area::new(33.0, -185.0, 34.0, -84.0).is_err());
        // a degree square at 33.5° is 110.5 km tall and 92.8 km wide: 500 × 110 540 / 92 828
        assert_eq!(area().height_for(500), 595);
    }

    #[test]
    fn slopes_facing_the_light_are_lighter_and_the_sea_is_blue() {
        // a ridge along the meridian at 84.5° W: its west face looks to the light, its east face away
        let ridge = |at: LatLon| Some(10_000.0 - 20_000.0 * (at.lon + 84.5).abs());
        let (w, h, px) = draw(area(), 100, &ridge);
        let pixel = |r: u32, c: u32| px[((r * w + c) * 4) as usize];
        let (row, west_face, east_face) = (h / 2, 30, 70);
        assert!(
            pixel(row, west_face) > pixel(row, east_face) + 40,
            "{} against {}",
            pixel(row, west_face),
            pixel(row, east_face)
        );
        // the sea, and no tile held
        let sea = |_: LatLon| Some(0.0);
        let (_, _, px) = draw(area(), 10, &sea);
        assert_eq!(&px[..4], &[178, 205, 228, 255]);
        let nothing = |_: LatLon| None;
        let (_, _, px) = draw(area(), 10, &nothing);
        assert!(px.chunks(4).all(|p| p[3] == 0), "transparent");
        // ground that rises to the north faces south, away from a north-west sun: darker
        let slope = |at: LatLon| Some(30_000.0 * (at.lat - 33.0));
        let (_, _, px) = draw(area(), 10, &slope);
        let rising_north = px[(5 * 10 + 5) * 4];
        let fall = |at: LatLon| Some(30_000.0 * (34.0 - at.lat));
        let (_, _, px) = draw(area(), 10, &fall);
        let rising_south = px[(5 * 10 + 5) * 4];
        assert!(
            rising_north + 40 < rising_south,
            "{rising_north} against {rising_south}"
        );
    }

    #[test]
    fn the_png_reads_back() {
        let rgba: Vec<u8> = (0..2 * 3 * 4)
            .map(|i| u8::try_from(i * 7 % 256).unwrap_or(0))
            .collect();
        let file = png(2, 3, &rgba);
        assert_eq!(&file[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&file[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(file[16..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_be_bytes(file[20..24].try_into().unwrap()), 3);
        // the header chunk's CRC, as ISO/IEC 15948 §5.5 has it over its type and data
        let mut crc = flate2::Crc::new();
        crc.update(&file[12..29]);
        assert_eq!(
            u32::from_be_bytes(file[29..33].try_into().unwrap()),
            crc.sum()
        );
        // the image data inflates to the rows, each after its filter byte
        let len = u32::from_be_bytes(file[33..37].try_into().unwrap()) as usize;
        assert_eq!(&file[37..41], b"IDAT");
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&file[41..41 + len])
            .read_to_end(&mut raw)
            .expect("zlib");
        assert_eq!(raw.len(), 3 * (1 + 8));
        assert_eq!(&raw[1..9], &rgba[..8]);
        assert_eq!(&file[file.len() - 8..file.len() - 4], b"IEND");
    }
}

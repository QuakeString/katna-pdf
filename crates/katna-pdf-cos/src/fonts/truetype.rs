// SPDX-License-Identifier: GPL-3.0-or-later

//! A TrueType font file read for embedding: which glyph each character
//! has, how wide each glyph is, what the file says about the face, and
//! the file written again holding only the glyphs a page uses (the rest
//! emptied, so glyph numbers stay as they were and the PDF's text can
//! name them directly).

use std::collections::{BTreeSet, HashMap};

pub(crate) struct TrueType {
    data: Vec<u8>,
    /// Each table's offset and length in `data`.
    tables: HashMap<[u8; 4], (usize, usize)>,
    pub units_per_em: f64,
    /// Characters to glyph numbers.
    cmap: HashMap<u32, u16>,
    /// Advance widths by glyph, in font units (the last repeated).
    advances: Vec<u16>,
    /// Where each glyph's outline starts in `glyf`, and the end of the
    /// last.
    loca: Vec<u32>,
    pub glyph_count: u16,
    /// Left, bottom, right, top, in font units.
    pub bbox: [i16; 4],
    pub ascent: i16,
    pub descent: i16,
    pub cap_height: i16,
    /// In degrees, anticlockwise from upright (so negative for a slant
    /// to the right).
    pub italic_angle: f64,
    pub weight: u16,
    pub fixed_pitch: bool,
    pub serif: bool,
    /// Whether the file's licence lets it be embedded (`OS/2` `fsType`
    /// without "restricted licence").
    pub embeddable: bool,
    pub postscript_name: String,
    pub family: String,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(at)?, *d.get(at + 1)?]))
}

fn i16_at(d: &[u8], at: usize) -> Option<i16> {
    u16_at(d, at).map(|v| v as i16)
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(at)?,
        *d.get(at + 1)?,
        *d.get(at + 2)?,
        *d.get(at + 3)?,
    ]))
}

impl TrueType {
    /// The font in `data`: face `index` of a collection, or the one
    /// font. None for a font without TrueType outlines (CFF) or one that
    /// cannot be read.
    pub fn parse(data: Vec<u8>, index: usize) -> Option<Self> {
        let mut at = 0;
        if data.get(..4) == Some(b"ttcf") {
            let count = u32_at(&data, 8)? as usize;
            if index >= count {
                return None;
            }
            at = u32_at(&data, 12 + 4 * index)? as usize;
        }
        match data.get(at..at + 4)? {
            [0, 1, 0, 0] | b"true" => {}
            _ => return None,
        }
        let count = u16_at(&data, at + 4)? as usize;
        let mut tables = HashMap::new();
        for i in 0..count {
            let rec = at + 12 + 16 * i;
            let tag: [u8; 4] = data.get(rec..rec + 4)?.try_into().ok()?;
            let offset = u32_at(&data, rec + 8)? as usize;
            let length = u32_at(&data, rec + 12)? as usize;
            if offset + length <= data.len() {
                tables.insert(tag, (offset, length));
            }
        }
        let table = |tag: &[u8; 4]| tables.get(tag).map(|&(o, l)| &data[o..o + l]);
        let head = table(b"head")?;
        let units_per_em = f64::from(u16_at(head, 18)?);
        let bbox = [
            i16_at(head, 36)?,
            i16_at(head, 38)?,
            i16_at(head, 40)?,
            i16_at(head, 42)?,
        ];
        let long_loca = i16_at(head, 50)? != 0;
        let maxp = table(b"maxp")?;
        let glyph_count = u16_at(maxp, 4)?;
        let hhea = table(b"hhea")?;
        let mut ascent = i16_at(hhea, 4)?;
        let mut descent = i16_at(hhea, 6)?;
        let metric_count = u16_at(hhea, 34)? as usize;
        let hmtx = table(b"hmtx")?;
        let advances: Vec<u16> = (0..metric_count)
            .filter_map(|i| u16_at(hmtx, 4 * i))
            .collect();
        let loca_table = table(b"loca")?;
        let loca: Vec<u32> = (0..=glyph_count as usize)
            .filter_map(|i| {
                if long_loca {
                    u32_at(loca_table, 4 * i)
                } else {
                    u16_at(loca_table, 2 * i).map(|v| u32::from(v) * 2)
                }
            })
            .collect();
        if loca.len() != glyph_count as usize + 1 {
            return None;
        }
        // A font cut down for a PDF has no `cmap`: glyphs are named by
        // number there.
        let cmap = table(b"cmap").and_then(read_cmap).unwrap_or_default();
        let mut weight = 400;
        let mut cap_height = 0;
        let mut embeddable = true;
        let mut serif = false;
        if let Some(os2) = table(b"OS/2") {
            weight = u16_at(os2, 4).unwrap_or(400);
            let fs_type = u16_at(os2, 8).unwrap_or(0);
            embeddable = fs_type & 0x0002 == 0;
            // PANOSE: family kind 2 (text) with a serif style other than
            // sans (11 to 15).
            if os2.get(32) == Some(&2) {
                serif = os2.get(33).is_some_and(|s| (2..=10).contains(s));
            }
            if u16_at(os2, 0).unwrap_or(0) >= 2 {
                cap_height = i16_at(os2, 88).unwrap_or(0);
            }
            if let (Some(a), Some(d)) = (i16_at(os2, 68), i16_at(os2, 70))
                && a != 0
            {
                ascent = a;
                descent = d;
            }
        }
        let mut italic_angle = 0.0;
        let mut fixed_pitch = false;
        if let Some(post) = table(b"post") {
            italic_angle = f64::from(u32_at(post, 4).unwrap_or(0) as i32) / 65536.0;
            fixed_pitch = u32_at(post, 12).unwrap_or(0) != 0;
        }
        let (family, postscript_name) = table(b"name").map(read_names).unwrap_or_default();
        let mut font = TrueType {
            data,
            tables,
            units_per_em,
            cmap,
            advances,
            loca,
            glyph_count,
            bbox,
            ascent,
            descent,
            cap_height,
            italic_angle,
            weight,
            fixed_pitch,
            serif,
            embeddable,
            postscript_name,
            family,
        };
        if font.cap_height == 0 {
            font.cap_height = font
                .gid('H')
                .and_then(|g| font.glyph_bbox(g))
                .map_or((0.7 * units_per_em) as i16, |b| b[3]);
        }
        if font.postscript_name.is_empty() {
            font.postscript_name = font.family.replace(' ', "");
        }
        if font.postscript_name.is_empty() {
            font.postscript_name = "Font".to_string();
        }
        Some(font)
    }

    fn table(&self, tag: &[u8; 4]) -> Option<&[u8]> {
        self.tables.get(tag).map(|&(o, l)| &self.data[o..o + l])
    }

    /// The glyph for `c`, when the font has one.
    pub fn gid(&self, c: char) -> Option<u16> {
        self.cmap.get(&(c as u32)).copied().filter(|&g| g != 0)
    }

    /// Glyph `gid`'s advance, in thousandths of the size.
    pub fn advance(&self, gid: u16) -> f64 {
        let i = (gid as usize).min(self.advances.len().saturating_sub(1));
        self.advances
            .get(i)
            .map_or(0.0, |&w| f64::from(w) * 1000.0 / self.units_per_em)
    }

    fn glyph_data(&self, gid: u16) -> Option<&[u8]> {
        let glyf = self.table(b"glyf")?;
        let start = *self.loca.get(gid as usize)? as usize;
        let end = *self.loca.get(gid as usize + 1)? as usize;
        glyf.get(start..end)
    }

    fn glyph_bbox(&self, gid: u16) -> Option<[i16; 4]> {
        let g = self.glyph_data(gid)?;
        Some([i16_at(g, 2)?, i16_at(g, 4)?, i16_at(g, 6)?, i16_at(g, 8)?])
    }

    /// The glyphs a composite glyph is made of.
    fn components(&self, gid: u16) -> Vec<u16> {
        let mut out = Vec::new();
        let Some(g) = self.glyph_data(gid) else {
            return out;
        };
        if i16_at(g, 0).is_none_or(|contours| contours >= 0) {
            return out;
        }
        let mut at = 10;
        while let (Some(flags), Some(component)) = (u16_at(g, at), u16_at(g, at + 2)) {
            out.push(component);
            at += 4;
            at += if flags & 0x0001 != 0 { 4 } else { 2 };
            if flags & 0x0008 != 0 {
                at += 2;
            } else if flags & 0x0040 != 0 {
                at += 4;
            } else if flags & 0x0080 != 0 {
                at += 8;
            }
            if flags & 0x0020 == 0 {
                break;
            }
        }
        out
    }

    /// The file with only glyphs `gids` (and the glyphs those are made
    /// of, and glyph 0) keeping their outlines: the tables a reader
    /// needs for glyphs named by number, the rest left out.
    pub fn subset(&self, gids: &[u16]) -> Vec<u8> {
        write_sfnt(self.subset_tables(gids))
    }

    /// `subset`, with a `cmap` table (Windows Unicode, format 4) mapping
    /// each character in `chars` to its glyph, as a simple font a reader
    /// looks glyphs up by character needs.
    pub fn subset_with_cmap(&self, gids: &[u16], chars: &[(u32, u16)]) -> Vec<u8> {
        let mut tables = self.subset_tables(gids);
        tables.push((*b"cmap", cmap_format4(chars)));
        tables.sort_by_key(|(tag, _)| *tag);
        write_sfnt(tables)
    }

    fn subset_tables(&self, gids: &[u16]) -> Vec<([u8; 4], Vec<u8>)> {
        let mut keep: BTreeSet<u16> = BTreeSet::new();
        let mut pending: Vec<u16> = gids.to_vec();
        pending.push(0);
        while let Some(g) = pending.pop() {
            if g < self.glyph_count && keep.insert(g) {
                pending.extend(self.components(g));
            }
        }
        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity(self.glyph_count as usize + 1);
        for g in 0..self.glyph_count {
            loca.push(glyf.len() as u32);
            if keep.contains(&g)
                && let Some(data) = self.glyph_data(g)
            {
                glyf.extend_from_slice(data);
                while glyf.len() % 4 != 0 {
                    glyf.push(0);
                }
            }
        }
        loca.push(glyf.len() as u32);
        let loca_bytes: Vec<u8> = loca.iter().flat_map(|v| v.to_be_bytes()).collect();
        let mut head = self.table(b"head").unwrap_or_default().to_vec();
        if head.len() >= 54 {
            head[8..12].copy_from_slice(&[0, 0, 0, 0]);
            head[50..52].copy_from_slice(&1u16.to_be_bytes());
        }
        let mut out_tables: Vec<([u8; 4], Vec<u8>)> =
            vec![(*b"head", head), (*b"loca", loca_bytes), (*b"glyf", glyf)];
        for tag in [
            b"hhea", b"hmtx", b"maxp", b"OS/2", b"cvt ", b"fpgm", b"prep",
        ] {
            if let Some(t) = self.table(tag) {
                out_tables.push((*tag, t.to_vec()));
            }
        }
        out_tables.sort_by_key(|(tag, _)| *tag);
        out_tables
    }
}

/// A `cmap` table with one Windows Unicode BMP subtable in format 4
/// mapping `chars` (code points below 0xFFFF) to glyphs.
fn cmap_format4(chars: &[(u32, u16)]) -> Vec<u8> {
    let mut pairs: Vec<(u16, u16)> = chars
        .iter()
        .filter_map(|(c, g)| {
            u16::try_from(*c)
                .ok()
                .filter(|c| *c < 0xFFFF)
                .map(|c| (c, *g))
        })
        .collect();
    pairs.sort_unstable();
    pairs.dedup_by_key(|p| p.0);
    // Runs of consecutive codes, each a segment whose glyphs are listed.
    let mut segments: Vec<(u16, u16, Vec<u16>)> = Vec::new();
    for (c, g) in pairs {
        match segments.last_mut() {
            Some((_, end, glyphs)) if *end + 1 == c => {
                *end = c;
                glyphs.push(g);
            }
            _ => segments.push((c, c, vec![g])),
        }
    }
    let seg_count = segments.len() + 1;
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut deltas = Vec::new();
    let mut offsets = Vec::new();
    let mut glyph_ids: Vec<u16> = Vec::new();
    for (i, (start, end, glyphs)) in segments.iter().enumerate() {
        starts.push(*start);
        ends.push(*end);
        deltas.push(0u16);
        // From this entry of the offsets to the glyphs' place.
        offsets.push((2 * (seg_count - i) + 2 * glyph_ids.len()) as u16);
        glyph_ids.extend(glyphs);
    }
    starts.push(0xFFFF);
    ends.push(0xFFFF);
    deltas.push(1);
    offsets.push(0);
    let mut sub = Vec::new();
    let length = 16 + 8 * seg_count + 2 * glyph_ids.len();
    let mut power = 1usize;
    let mut log = 0u16;
    while power * 2 <= seg_count {
        power *= 2;
        log += 1;
    }
    sub.extend_from_slice(&4u16.to_be_bytes());
    sub.extend_from_slice(&(length as u16).to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes());
    sub.extend_from_slice(&((seg_count * 2) as u16).to_be_bytes());
    sub.extend_from_slice(&((power * 2) as u16).to_be_bytes());
    sub.extend_from_slice(&log.to_be_bytes());
    sub.extend_from_slice(&(((seg_count - power) * 2) as u16).to_be_bytes());
    for list in [&ends] {
        for v in list.iter() {
            sub.extend_from_slice(&v.to_be_bytes());
        }
    }
    sub.extend_from_slice(&0u16.to_be_bytes());
    for list in [&starts, &deltas, &offsets, &glyph_ids] {
        for v in list.iter() {
            sub.extend_from_slice(&v.to_be_bytes());
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&3u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&12u32.to_be_bytes());
    out.extend_from_slice(&sub);
    out
}

/// Characters to glyphs from the best `cmap` subtable: full Unicode
/// (format 12), else the basic plane (format 4).
fn read_cmap(cmap: &[u8]) -> Option<HashMap<u32, u16>> {
    let count = u16_at(cmap, 2)? as usize;
    let mut best: Option<(u8, usize)> = None;
    for i in 0..count {
        let rec = 4 + 8 * i;
        let platform = u16_at(cmap, rec)?;
        let encoding = u16_at(cmap, rec + 2)?;
        let offset = u32_at(cmap, rec + 4)? as usize;
        let format = u16_at(cmap, offset)?;
        let rank = match (platform, encoding, format) {
            (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
            (3, 1, 4) => 2,
            (0, _, 4) => 1,
            _ => continue,
        };
        if best.is_none_or(|(r, _)| rank > r) {
            best = Some((rank, offset));
        }
    }
    let (_, at) = best?;
    let mut map = HashMap::new();
    match u16_at(cmap, at)? {
        12 => {
            let groups = u32_at(cmap, at + 12)? as usize;
            for g in 0..groups.min(100_000) {
                let rec = at + 16 + 12 * g;
                let start = u32_at(cmap, rec)?;
                let end = u32_at(cmap, rec + 4)?;
                let glyph = u32_at(cmap, rec + 8)?;
                for (k, c) in (start..=end.min(start + 0xFFFF)).enumerate() {
                    let gid = glyph + k as u32;
                    if gid <= 0xFFFF {
                        map.insert(c, gid as u16);
                    }
                }
            }
        }
        4 => {
            let seg_x2 = u16_at(cmap, at + 6)? as usize;
            let segs = seg_x2 / 2;
            let ends = at + 14;
            let starts = ends + seg_x2 + 2;
            let deltas = starts + seg_x2;
            let range_offsets = deltas + seg_x2;
            for s in 0..segs {
                let end = u16_at(cmap, ends + 2 * s)?;
                let start = u16_at(cmap, starts + 2 * s)?;
                let delta = u16_at(cmap, deltas + 2 * s)?;
                let range_offset = u16_at(cmap, range_offsets + 2 * s)? as usize;
                if start > end {
                    continue;
                }
                for c in start..=end {
                    if c == 0xFFFF {
                        break;
                    }
                    let gid = if range_offset == 0 {
                        c.wrapping_add(delta)
                    } else {
                        let addr = range_offsets + 2 * s + range_offset + 2 * (c - start) as usize;
                        match u16_at(cmap, addr) {
                            Some(0) | None => 0,
                            Some(g) => g.wrapping_add(delta),
                        }
                    };
                    if gid != 0 {
                        map.insert(u32::from(c), gid);
                    }
                }
            }
        }
        _ => return None,
    }
    Some(map)
}

/// The family and PostScript names from the `name` table.
fn read_names(name: &[u8]) -> (String, String) {
    let count = u16_at(name, 2).unwrap_or(0) as usize;
    let strings = u16_at(name, 4).unwrap_or(0) as usize;
    let mut family = None;
    let mut postscript = None;
    for i in 0..count {
        let rec = 6 + 12 * i;
        let (Some(platform), Some(encoding), Some(id), Some(len), Some(off)) = (
            u16_at(name, rec),
            u16_at(name, rec + 2),
            u16_at(name, rec + 6),
            u16_at(name, rec + 8),
            u16_at(name, rec + 10),
        ) else {
            break;
        };
        let Some(bytes) = name.get(strings + off as usize..strings + (off + len) as usize) else {
            continue;
        };
        let text = match (platform, encoding) {
            (3, 1) | (3, 10) | (0, _) => {
                let units: Vec<u16> = bytes
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            (1, 0) => bytes.iter().map(|&b| b as char).collect(),
            _ => continue,
        };
        let slot = match id {
            1 => &mut family,
            6 => &mut postscript,
            _ => continue,
        };
        // Windows' Unicode names over Macintosh ones.
        if slot.is_none() || platform == 3 {
            *slot = Some(text);
        }
    }
    let ascii = |s: String| -> String {
        s.chars()
            .filter(|c| {
                c.is_ascii_graphic()
                    && !matches!(c, '[' | ']' | '(' | ')' | '{' | '}' | '<' | '>' | '/' | '%')
            })
            .collect()
    };
    (
        family.unwrap_or_default().trim().to_string(),
        ascii(postscript.unwrap_or_default()),
    )
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4)
        .map(|c| {
            u32::from_be_bytes([
                c[0],
                *c.get(1).unwrap_or(&0),
                *c.get(2).unwrap_or(&0),
                *c.get(3).unwrap_or(&0),
            ])
        })
        .fold(0u32, u32::wrapping_add)
}

/// A font file of `tables` (sorted by tag), with its directory and
/// checksums.
fn write_sfnt(tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    let count = tables.len() as u16;
    let mut power = 1u16;
    let mut log = 0u16;
    while power * 2 <= count {
        power *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 1, 0, 0]);
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&(power * 16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&((count - power) * 16).to_be_bytes());
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(offset);
        }
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(at) = head_at
        && out.len() >= at + 12
    {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjust.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dejavu() -> Option<TrueType> {
        let data = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf").ok()?;
        TrueType::parse(data, 0)
    }

    #[test]
    fn a_font_is_read_and_cut_down() {
        let Some(font) = dejavu() else {
            eprintln!("DejaVu Sans not installed; skipped");
            return;
        };
        assert_eq!(font.family, "DejaVu Sans");
        assert_eq!(font.postscript_name, "DejaVuSans");
        assert_eq!(font.units_per_em, 2048.0);
        assert!(font.embeddable);
        assert!(!font.serif);
        let a = font.gid('a').unwrap();
        assert!((font.advance(a) - 613.0).abs() < 1.0, "{}", font.advance(a));
        assert!(font.gid('é').is_some());
        assert!(font.gid('\u{10FFFF}').is_none());
        assert!(font.cap_height > 1000 && font.ascent > 1000 && font.descent < 0);

        let gids: Vec<u16> = "Hello".chars().filter_map(|c| font.gid(c)).collect();
        let small = font.subset(&gids);
        // Glyphs keep their numbers, so the glyph and metrics tables
        // stay as long as the font has glyphs; the outlines are few.
        assert!(small.len() < 60_000, "{}", small.len());
        let again = TrueType::parse(small, 0).unwrap();
        assert_eq!(again.glyph_count, font.glyph_count);
        assert!((again.advance(a) - font.advance(a)).abs() < 1e-9);
        let h = font.gid('H').unwrap();
        assert_eq!(again.glyph_bbox(h), font.glyph_bbox(h));
        assert!(again.glyph_bbox(font.gid('z').unwrap()).is_none());
    }
}

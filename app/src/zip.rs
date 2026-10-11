//! A zip written as it goes, for a download too big to hold: each file
//! stored as it is (photos and films are compressed already), its CRC
//! worked out as its bytes pass, its sizes in a descriptor after it, and
//! zip64 wherever a size, an offset or the count outgrows the old fields.
//! The same format as the site's (servers/home/web/zip.js). Each call hands
//! back the bytes to send next; nothing is kept but the directory at the end.

const MAX32: u64 = 0xFFFF_FFFF;
const UTF8: u16 = 0x0800;
const LATER: u16 = 0x0008;

static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
    let mut t = [0u32; 256];
    for (n, slot) in t.iter_mut().enumerate() {
        let mut c = n as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *slot = c;
    }
    t
});

fn crc32(crc: u32, bytes: &[u8]) -> u32 {
    let t = &*TABLE;
    let mut c = !crc;
    for b in bytes {
        c = t[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

/// A name inside the zip: forward slashes, nothing that climbs out of the
/// folder it is unpacked into, nothing empty
pub fn safe_name(path: &str) -> String {
    let parts: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .map(|p| if p == ".." { "_" } else { p })
        .collect();
    if parts.is_empty() {
        "_".into()
    } else {
        parts.join("/")
    }
}

/// MS-DOS time, as zip keeps it, from seconds since the epoch (UTC)
fn dos(secs: i64) -> (u16, u16) {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    if !(1980..=2107).contains(&y) {
        return (0, (1 << 5) | 1);
    }
    let time = ((rem / 3600) << 11) | (((rem / 60) % 60) << 5) | ((rem % 60) / 2);
    let date = ((y - 1980) << 9) | (m << 5) | d;
    (time as u16, date as u16)
}

struct Entry {
    name: Vec<u8>,
    time: u16,
    date: u16,
    offset: u64,
    crc: u32,
    size: u64,
    dir: bool,
}

#[derive(Default)]
pub struct Zip {
    at: u64,
    entries: Vec<Entry>,
    names: std::collections::HashSet<String>,
    open: Option<Entry>,
}

impl Zip {
    pub fn new() -> Zip {
        Zip::default()
    }

    fn unique(&mut self, name: String) -> String {
        let mut n = name.clone();
        let mut k = 2;
        while self.names.contains(&n) {
            let slash = name.rfind('/').map_or(0, |i| i + 1);
            n = match name.rfind('.') {
                Some(dot) if dot > slash => format!("{} ({k}){}", &name[..dot], &name[dot..]),
                _ => format!("{name} ({k})"),
            };
            k += 1;
        }
        self.names.insert(n.clone());
        n
    }

    fn header(&mut self, name: String, when: i64, dir: bool) -> Vec<u8> {
        let (time, date) = dos(when);
        let name = name.into_bytes();
        let mut h = Vec::with_capacity(30 + name.len() + 20);
        h.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        h.extend_from_slice(&45u16.to_le_bytes());
        h.extend_from_slice(&(UTF8 | LATER).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&time.to_le_bytes());
        h.extend_from_slice(&date.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&(MAX32 as u32).to_le_bytes());
        h.extend_from_slice(&(MAX32 as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&name);
        // the sizes come after the data, eight bytes each (zip64)
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&16u16.to_le_bytes());
        h.extend_from_slice(&[0u8; 16]);
        self.open = Some(Entry {
            name,
            time,
            date,
            offset: self.at,
            crc: 0,
            size: 0,
            dir,
        });
        self.at += h.len() as u64;
        h
    }

    /// a folder: its header and descriptor at once
    pub fn folder(&mut self, path: &str, when: i64) -> Vec<u8> {
        let name = self.unique(format!("{}/", safe_name(path)));
        let mut out = self.header(name, when, true);
        out.extend(self.done());
        out
    }

    /// a file's header; then `data` for its bytes, then `done`
    pub fn file(&mut self, path: &str, when: i64) -> Vec<u8> {
        let name = self.unique(safe_name(path));
        self.header(name, when, false)
    }

    /// counts a piece of the open file as it goes out
    pub fn data(&mut self, piece: &[u8]) {
        if let Some(e) = self.open.as_mut() {
            e.crc = crc32(e.crc, piece);
            e.size += piece.len() as u64;
            self.at += piece.len() as u64;
        }
    }

    /// the open file's descriptor
    pub fn done(&mut self) -> Vec<u8> {
        let Some(e) = self.open.take() else {
            return Vec::new();
        };
        let mut d = Vec::with_capacity(24);
        d.extend_from_slice(&0x0807_4B50u32.to_le_bytes());
        d.extend_from_slice(&e.crc.to_le_bytes());
        d.extend_from_slice(&e.size.to_le_bytes());
        d.extend_from_slice(&e.size.to_le_bytes());
        self.at += d.len() as u64;
        self.entries.push(e);
        d
    }

    /// the directory and its end
    pub fn end(&mut self) -> Vec<u8> {
        let start = self.at;
        let mut out = Vec::new();
        for e in &self.entries {
            let big = [e.size >= MAX32, e.size >= MAX32, e.offset >= MAX32];
            let extra = big.iter().filter(|b| **b).count() as u16 * 8;
            let ext_len = if extra > 0 { 4 + extra } else { 0 };
            let attrs: u32 = if e.dir {
                (0o40755 << 16) | 0x10
            } else {
                0o100644 << 16
            };
            out.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
            out.extend_from_slice(&((3u16 << 8) | 45).to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&(UTF8 | LATER).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&e.time.to_le_bytes());
            out.extend_from_slice(&e.date.to_le_bytes());
            out.extend_from_slice(&e.crc.to_le_bytes());
            let small = |v: u64, b: bool| if b { MAX32 as u32 } else { v as u32 };
            out.extend_from_slice(&small(e.size, big[1]).to_le_bytes());
            out.extend_from_slice(&small(e.size, big[0]).to_le_bytes());
            out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            out.extend_from_slice(&ext_len.to_le_bytes());
            out.extend_from_slice(&[0u8; 6]);
            out.extend_from_slice(&attrs.to_le_bytes());
            out.extend_from_slice(&small(e.offset, big[2]).to_le_bytes());
            out.extend_from_slice(&e.name);
            if extra > 0 {
                out.extend_from_slice(&1u16.to_le_bytes());
                out.extend_from_slice(&extra.to_le_bytes());
                if big[0] {
                    out.extend_from_slice(&e.size.to_le_bytes());
                }
                if big[1] {
                    out.extend_from_slice(&e.size.to_le_bytes());
                }
                if big[2] {
                    out.extend_from_slice(&e.offset.to_le_bytes());
                }
            }
        }
        let size = out.len() as u64;
        let n = self.entries.len() as u64;
        if n >= 0xFFFF || size >= MAX32 || start >= MAX32 {
            let z = start + size;
            out.extend_from_slice(&0x0606_4B50u32.to_le_bytes());
            out.extend_from_slice(&44u64.to_le_bytes());
            out.extend_from_slice(&((3u16 << 8) | 45).to_le_bytes());
            out.extend_from_slice(&45u16.to_le_bytes());
            out.extend_from_slice(&[0u8; 8]);
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&start.to_le_bytes());
            out.extend_from_slice(&0x0706_4B50u32.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&z.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
        }
        out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]);
        out.extend_from_slice(&(n.min(0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&(n.min(0xFFFF) as u16).to_le_bytes());
        out.extend_from_slice(&(size.min(MAX32) as u32).to_le_bytes());
        out.extend_from_slice(&(start.min(MAX32) as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        self.at += out.len() as u64;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_is_zips() {
        assert_eq!(crc32(0, b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(crc32(0, b"1234"), b"56789"), 0xCBF4_3926);
    }

    #[test]
    fn names_stay_inside_their_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "_/_/etc/passwd");
        assert_eq!(safe_name("/a//b\\c/./d"), "a/b/c/d");
        assert_eq!(safe_name(""), "_");
    }

    #[test]
    fn dos_time() {
        // 2024-06-01 12:30:10 UTC
        assert_eq!(
            dos(1_717_245_010),
            ((12 << 11) | (30 << 5) | 5, (44 << 9) | (6 << 5) | 1)
        );
    }

    /// a zip made here, opened by Python's zipfile (when there is one):
    /// names, folders, the same name twice, contents
    #[test]
    fn a_zip_opens_elsewhere() {
        let mut z = Zip::new();
        let mut out = z.folder("Holiday", 1_717_245_010);
        out.extend(z.file("Holiday/été.jpg", 1_717_245_010));
        for piece in [&b"abc"[..], b"def"] {
            z.data(piece);
            out.extend_from_slice(piece);
        }
        out.extend(z.done());
        out.extend(z.file("Holiday/été.jpg", 0));
        z.data(b"x");
        out.push(b'x');
        out.extend(z.done());
        out.extend(z.end());
        assert_eq!(z.at, out.len() as u64);
        let dir = std::env::temp_dir().join(format!("commonty-zip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("t.zip");
        std::fs::write(&f, &out).unwrap();
        let py = "import sys,zipfile\nz=zipfile.ZipFile(sys.argv[1])\nassert z.testzip() is None\n\
                  assert z.namelist()==['Holiday/','Holiday/été.jpg','Holiday/été (2).jpg'],z.namelist()\n\
                  assert z.read('Holiday/été.jpg')==b'abcdef'\nassert z.read('Holiday/été (2).jpg')==b'x'\n";
        match std::process::Command::new("python3")
            .arg("-c")
            .arg(py)
            .arg(&f)
            .output()
        {
            Ok(o) => assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr)),
            Err(_) => eprintln!("no python3 here: the zip was not opened elsewhere"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

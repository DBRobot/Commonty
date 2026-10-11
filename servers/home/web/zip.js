// A zip written as it goes, for a download too big to hold: each file is
// stored as it is (no compression: films and photos are compressed
// already), its CRC worked out while its bytes pass through, and its sizes
// in a descriptor after it. Zip64 wherever a size, an offset or the count
// outgrows the old fields, so a library of any size opens. The same format
// as the app's photo zip (app/src/zip.rs).

const TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xEDB88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(crc, bytes) {
  let c = ~crc >>> 0;
  for (let i = 0; i < bytes.length; i++) c = TABLE[(c ^ bytes[i]) & 0xFF] ^ (c >>> 8);
  return ~c >>> 0;
}

const MAX32 = 0xFFFFFFFF;
const UTF8 = 0x0800;
const LATER = 0x0008;

class Bytes {
  constructor(n) { this.b = new Uint8Array(n); this.v = new DataView(this.b.buffer); this.i = 0; }
  u16(x) { this.v.setUint16(this.i, x, true); this.i += 2; return this; }
  u32(x) { this.v.setUint32(this.i, x >>> 0, true); this.i += 4; return this; }
  u64(x) { this.v.setBigUint64(this.i, BigInt(x), true); this.i += 8; return this; }
  raw(a) { this.b.set(a, this.i); this.i += a.length; return this; }
}

// MS-DOS time, as zip keeps it: local time, two-second steps, from 1980
function dos(date) {
  const d = date instanceof Date && !Number.isNaN(date.getTime()) && date.getFullYear() >= 1980 ? date : new Date(1980, 0, 1);
  return {
    time: (d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1),
    date: ((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate(),
  };
}

/// A name inside the zip: forward slashes, nothing that climbs out of the
/// folder it is unpacked into, nothing empty
export function safeName(path) {
  return String(path)
    .replace(/\\/g, '/')
    .split('/')
    .filter((p) => p && p !== '.')
    .map((p) => (p === '..' ? '_' : p))
    .join('/') || '_';
}

/// `write(bytes)` is where the zip goes, awaited: a file, a download, a
/// list of pieces. Call `file` for each file in turn with an async source of
/// its bytes, `folder` for each folder, then `end`.
export class ZipWriter {
  constructor(write) {
    this.write = write;
    this.at = 0;
    this.entries = [];
    this.names = new Set();
  }

  async out(bytes) {
    await this.write(bytes);
    this.at += bytes.length;
  }

  // the same name twice gets " (2)" before its extension
  unique(name) {
    let n = name;
    for (let k = 2; this.names.has(n); k++) {
      const slash = name.lastIndexOf('/');
      const dot = name.lastIndexOf('.');
      n = dot > slash + 1 ? `${name.slice(0, dot)} (${k})${name.slice(dot)}` : `${name} (${k})`;
    }
    this.names.add(n);
    return n;
  }

  async header(name, when, dir) {
    const enc = new TextEncoder().encode(name);
    const { time, date } = dos(when);
    const e = { enc, time, date, offset: this.at, crc: 0, size: 0, dir };
    // the sizes come after the data, in eight bytes each (zip64), so the
    // header says so with an extra field of zeros
    const h = new Bytes(30 + enc.length + 20)
      .u32(0x04034B50).u16(45).u16(UTF8 | LATER).u16(0)
      .u16(time).u16(date).u32(0).u32(MAX32).u32(MAX32)
      .u16(enc.length).u16(20).raw(enc)
      .u16(1).u16(16).u64(0).u64(0);
    await this.out(h.b);
    return e;
  }

  async descriptor(e) {
    await this.out(new Bytes(24).u32(0x08074B50).u32(e.crc).u64(e.size).u64(e.size).b);
    this.entries.push(e);
  }

  async folder(path, when) {
    const name = this.unique(safeName(path) + '/');
    await this.descriptor(await this.header(name, when, true));
  }

  /// `source` is an async iterable of Uint8Array; `signal` stops it
  async file(path, when, source) {
    const e = await this.header(this.unique(safeName(path)), when, false);
    for await (const piece of source) {
      e.crc = crc32(e.crc, piece);
      e.size += piece.length;
      await this.out(piece);
    }
    await this.descriptor(e);
  }

  async end() {
    const start = this.at;
    for (const e of this.entries) {
      const big = [e.size >= MAX32, e.size >= MAX32, e.offset >= MAX32];
      const extra = big.filter(Boolean).length * 8;
      const c = new Bytes(46 + e.enc.length + (extra ? 4 + extra : 0))
        .u32(0x02014B50).u16((3 << 8) | 45).u16(45).u16(UTF8 | LATER).u16(0)
        .u16(e.time).u16(e.date).u32(e.crc)
        .u32(big[1] ? MAX32 : e.size).u32(big[0] ? MAX32 : e.size)
        .u16(e.enc.length).u16(extra ? 4 + extra : 0).u16(0).u16(0).u16(0)
        .u32((e.dir ? 0o40755 : 0o100644) * 0x10000 + (e.dir ? 0x10 : 0))
        .u32(big[2] ? MAX32 : e.offset)
        .raw(e.enc);
      if (extra) {
        c.u16(1).u16(extra);
        if (big[0]) c.u64(e.size);
        if (big[1]) c.u64(e.size);
        if (big[2]) c.u64(e.offset);
      }
      await this.out(c.b);
    }
    const size = this.at - start;
    const n = this.entries.length;
    if (n >= 0xFFFF || size >= MAX32 || start >= MAX32) {
      const z = this.at;
      await this.out(new Bytes(56 + 20)
        .u32(0x06064B50).u64(44).u16((3 << 8) | 45).u16(45).u32(0).u32(0)
        .u64(n).u64(n).u64(size).u64(start)
        .u32(0x07064B50).u32(0).u64(z).u32(1).b);
    }
    await this.out(new Bytes(22)
      .u32(0x06054B50).u16(0).u16(0)
      .u16(Math.min(n, 0xFFFF)).u16(Math.min(n, 0xFFFF))
      .u32(Math.min(size, MAX32)).u32(Math.min(start, MAX32)).u16(0).b);
  }
}

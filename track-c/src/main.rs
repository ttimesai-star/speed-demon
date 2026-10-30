// Speed Demon 2026, track C (Join). Spec: specs/c-join.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering::{Acquire, Relaxed, Release}};
use std::{env, fs, thread};

#[cfg(unix)]
fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
        fn prctl(op: i32, ...) -> i32;
    }
    unsafe { prctl(41, 1u64, 0u64, 0u64, 0u64) }; // PR_SET_THP_DISABLE: sparse bitsets stay sparse
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}

#[cfg(not(unix))]
fn load(path: &str) -> &'static [u8] {
    fs::read(path).unwrap().leak()
}

const SLOTS: usize = 1 << 18; // name table; the generator's pool has 100,000 names (ids < 2^20)
const IDS: usize = 1 << 20; // bits per country bitset, only the touched pages cost memory

// Zeroed atomics from calloc'd memory, so untouched pages are never faulted in.
fn zeroed(n: usize) -> &'static [AtomicU64] {
    let v: &'static mut [u64] = vec![0u64; n].leak();
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const AtomicU64, n) }
}

// Steps over the decimal digits at d[*p..] and returns their value, eight digits at a time: a digit has
// bit 4 set, the separators that follow numbers here (, . CR LF) and the zero padding have it clear.
fn num(d: &[u8], p: &mut usize) -> u64 {
    let w = word(d, *p);
    let k = (!w & 0x1010_1010_1010_1010).trailing_zeros() as usize / 8;
    let x = (w << (64 - 8 * k) % 64) & 0x0F0F_0F0F_0F0F_0F0F & (k > 0) as u64 * u64::MAX;
    let x = (x.wrapping_mul(10) + (x >> 8)) & 0x00FF_00FF_00FF_00FF;
    let x = (x.wrapping_mul(100) + (x >> 16)) & 0x0000_FFFF_0000_FFFF;
    let mut x = (x.wrapping_mul(10000) + (x >> 32)) & 0xFFFF_FFFF;
    *p += k;
    while k == 8 && d[*p].is_ascii_digit() {
        x = x * 10 + (d[*p] - b'0') as u64;
        *p += 1;
    }
    x
}

// Eight bytes at d[p..], little-endian, zero-padded past the end of d.
fn word(d: &[u8], p: usize) -> u64 {
    if p + 8 <= d.len() {
        return u64::from_le(unsafe { (d.as_ptr().add(p) as *const u64).read_unaligned() });
    }
    let mut b = [0u8; 8];
    b[..d.len() - p].copy_from_slice(&d[p..]);
    u64::from_le_bytes(b)
}

// Offset of the first byte c in d[p..], eight bytes at a time (exact zero-byte test).
fn find(d: &[u8], mut p: usize, c: u8) -> usize {
    loop {
        let x = word(d, p) ^ c as u64 * 0x0101_0101_0101_0101;
        let z = !(((x & 0x7F7F_7F7F_7F7F_7F7F) + 0x7F7F_7F7F_7F7F_7F7F) | x) & 0x8080_8080_8080_8080;
        if z != 0 {
            return p + z.trailing_zeros() as usize / 8;
        }
        p += 8;
    }
}

// Parses one text field at d[*p..] and steps over it and the comma after it: (raw start, raw length,
// number of "" pairs). For a quoted field the raw bytes are those between the delimiting quotes.
fn field(d: &[u8], p: &mut usize) -> (usize, usize, usize) {
    let (s, mut q, mut pairs) = (*p + 1, *p + 1, 0);
    if d[*p] != b'"' {
        let e = find(d, *p, b',');
        let r = (*p, e - *p, 0);
        *p = e + 1;
        return r;
    }
    loop {
        q = find(d, q, b'"');
        if d[q + 1] != b'"' {
            *p = q + 2;
            return (s, q - s, pairs);
        }
        (pairs, q) = (pairs + 1, q + 2);
    }
}

// Dense id of the raw name d[s..s+n], shared by all threads. A new name is copied to the arena first,
// then published by CAS as arena offset << 36 | length << 20 | id (bit 63 set, 0 = empty slot).
// Hash of the raw name d[s..s+n] and its first slot in the name table.
fn name_slot(d: &[u8], s: usize, n: usize) -> usize {
    let mut h = n as u64;
    for k in (0..n).step_by(8) {
        let w = word(d, s + k) & u64::MAX >> 8 * (8 - (n - k).min(8)) as u32 % 64;
        h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    (h >> 40) as usize & (SLOTS - 1)
}

// Dense id of the raw name d[s..s+n] whose first slot is i (see name_slot).
fn name_id(tab: &[AtomicU64], arena: &[AtomicU64], top: &AtomicU64, d: &[u8], s: usize, n: usize, mut i: usize) -> u64 {
    loop {
        let mut v = tab[i].load(Acquire);
        if v == 0 {
            let words = n.div_ceil(8) as u64;
            let o = top.fetch_add(words << 20 | 1, Relaxed); // arena words << 20 | ids
            let (at, id) = ((o >> 20) as usize, (o & 0xFFFFF) + 1);
            for k in 0..words as usize {
                let w = word(d, s + 8 * k) & u64::MAX >> 8 * (8 - (n - 8 * k).min(8)) as u32 % 64;
                arena[at + k].store(w, Relaxed);
            }
            let key = 1 << 63 | (at as u64) << 36 | (n as u64) << 20 | id;
            match tab[i].compare_exchange(0, key, Release, Acquire) {
                Ok(_) => return id,
                Err(w) => v = w,
            }
        }
        let (at, l) = ((v >> 36 & 0x7FF_FFFF) as usize, (v >> 20 & 0xFFFF) as usize);
        if l == n
            && (0..n.div_ceil(8)).all(|k| {
                arena[at + k].load(Relaxed) == word(d, s + 8 * k) & u64::MAX >> 8 * (8 - (n - 8 * k).min(8)) as u32 % 64
            })
        {
            return v & 0xFFFFF;
        }
        i = (i + 1) & (SLOTS - 1);
    }
}

#[derive(Clone, Copy)]
struct Agg {
    count: i64,
    ids: i64,
    sum: i64,
    min: i64,
    max: i64,
    name_bytes: i64,
    note_bytes: i64,
}

const ZERO: Agg = Agg { count: 0, ids: 0, sum: 0, min: i64::MAX, max: i64::MIN, name_bytes: 0, note_bytes: 0 };

fn money(out: &mut Vec<u8>, c: i64) {
    let sign = if c < 0 { "-" } else { "" };
    write!(out, " {sign}{}.{:02}", c.abs() / 100, c.abs() % 100).unwrap();
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    // regions.csv: region id -> country index, countries in byte order. A code is a base-26 number, so the
    // order of the numbers is the byte order and a 26^3 table replaces a sort. Two passes over the mapped
    // file instead of a list of rows.
    let r = load(&format!("{din}/regions.csv"));
    let rows = |f: &mut dyn FnMut(usize, usize)| {
        let mut p = r.iter().position(|&c| c == b'\n').unwrap() + 1;
        while p < r.len() {
            let id = num(r, &mut p) as usize;
            (f(id, r[p + 1..p + 4].iter().fold(0, |k, &c| k * 26 + (c - b'A') as usize)), p += 5);
        }
    };
    let (mut idx, mut top) = (vec![u16::MAX; 26 * 26 * 26], 0);
    rows(&mut |id, key| (idx[key] = 0, top = top.max(id)).1);
    let mut codes = vec![];
    for key in 0..idx.len() {
        if idx[key] == 0 {
            (idx[key] = codes.len() as u16, codes.push([key / 676, key / 26 % 26, key % 26].map(|x| b'A' + x as u8)));
        }
    }
    let nc = codes.len(); // index nc is _UNMATCHED
    assert!(nc < 256);
    let mut country = vec![nc as u8; top + 1];
    rows(&mut |id, key| country[id] = idx[key] as u8);
    let d = load(&format!("{din}/records.csv"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    // Chunk starts without a pass over the file: by section 2.2 a note is lowercase words (an LF in it is
    // followed by a letter, a quote or a space-free word) and a name has no LF, so an LF followed by a digit
    // or by the first byte of a BOM always ends a record.
    let mut bs = vec![d.iter().position(|&c| c == b'\n').map_or(d.len(), |x| x + 1)]; // after the header
    for k in 1..nt {
        let from = (k * d.len() / nt).max(bs[k - 1]).max(1);
        let start = (from..d.len()).find(|&i| d[i - 1] == b'\n' && (d[i].is_ascii_digit() || d[i] == 0xEF));
        bs.push(start.unwrap_or(d.len()));
    }
    bs.push(d.len());
    let (tab, arena, top) = (zeroed(SLOTS), zeroed(1 << 24), AtomicU64::new(0));
    let seen = zeroed((nc + 1) * IDS / 64);
    let (bs, country, top) = (&bs, &country, &top);
    let parts: Vec<Vec<Agg>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|k| {
                sc.spawn(move || {
                    let mut agg = vec![ZERO; nc + 1];
                    let (mut pend, mut bit) = (None, None);
                    let set = |(k, b): (usize, u64)| {
                        if seen[k].load(Relaxed) & b == 0 {
                            seen[k].fetch_or(b, Relaxed);
                        }
                    };
                    // Stage 2 of 3: the name's id, then a prefetch of its word in the country's bitset; the
                    // bit itself is set one record later (stage 3).
                    let mut mark = |(ns, nn, i, g): (usize, usize, usize, usize)| {
                        let nid = if nn == 0 { 0 } else { name_id(tab, arena, top, d, ns, nn, i) } as usize;
                        let k = g * IDS / 64 + nid / 64;
                        unsafe { std::arch::x86_64::_mm_prefetch::<3>(seen.as_ptr().add(k) as *const i8) };
                        bit.replace((k, 1u64 << (nid % 64))).map(set);
                    };
                    let mut p = bs[k];
                    while p < bs[k + 1] {
                        if d[p..].starts_with(&[0xEF, 0xBB, 0xBF]) {
                            p += 3;
                        }
                        let id = num(d, &mut p) as i64;
                        p += 1;
                        let (ns, nn, np) = field(d, &mut p);
                        let (_, tn, tp) = field(d, &mut p);
                        let reg = num(d, &mut p) as usize;
                        p += 1;
                        let neg = d[p] == b'-';
                        p += neg as usize;
                        let whole = num(d, &mut p) as i64;
                        let cents = (d[p + 1] - b'0') as i64 * 10 + (d[p + 2] - b'0') as i64;
                        p += 3;
                        p += if d[p] == b'\r' { 2 } else { 1 };
                        let amt = if neg { -(whole * 100 + cents) } else { whole * 100 + cents };
                        let g = *country.get(reg).unwrap_or(&(nc as u8)) as usize;
                        let a = &mut agg[g];
                        (a.count += 1, a.ids += id, a.sum += amt);
                        (a.min = a.min.min(amt), a.max = a.max.max(amt));
                        (a.name_bytes += (nn - np) as i64, a.note_bytes += (tn - tp) as i64);
                        // A two-stage pipeline: this record's slot is prefetched, the previous record's name is
                        // looked up (its slot is in cache by now) and marked for its country.
                        let i = name_slot(d, ns, nn);
                        unsafe { std::arch::x86_64::_mm_prefetch::<3>(tab.as_ptr().add(i) as *const i8) };
                        if let Some(x) = pend.replace((ns, nn, i, g)) {
                            mark(x);
                        }
                    }
                    pend.map(&mut mark);
                    bit.map(set);
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(((top.load(Relaxed) & 0xFFFFF) as usize) < IDS && (top.load(Relaxed) >> 20) < 1 << 24);
    let mut out = Vec::new();
    for g in 0..=nc {
        let mut a = ZERO;
        for b in parts.iter().map(|x| x[g]) {
            (a.count += b.count, a.ids += b.ids, a.sum += b.sum, a.min = a.min.min(b.min));
            (a.max = a.max.max(b.max), a.name_bytes += b.name_bytes, a.note_bytes += b.note_bytes);
        }
        if a.count == 0 {
            continue;
        }
        let used = (top.load(Relaxed) & 0xFFFFF) as usize / 64 + 1; // words that can hold a set bit
        let names: u32 = seen[g * IDS / 64..][..used].iter().map(|w| w.load(Relaxed).count_ones()).sum();
        out.extend_from_slice(if g < nc { &codes[g][..] } else { b"_UNMATCHED" });
        write!(out, " {} {}", a.count, a.ids).unwrap();
        for c in [a.sum, a.min, a.max] {
            money(&mut out, c);
        }
        writeln!(out, " {} {} {}", names, a.name_bytes, a.note_bytes).unwrap();
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/aggregates.txt"), out).unwrap();
}

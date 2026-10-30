// Speed Demon 2026, track C (Join). Spec: specs/c-join.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
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

const SLOTS: usize = 1 << 19; // name table; the generator's pool has 100,000 names
const IDS: usize = 1 << 20; // bits per country bitset, only the touched pages cost memory

// Zeroed atomics from calloc'd memory, so untouched pages are never faulted in.
fn zeroed(n: usize) -> &'static [AtomicU64] {
    let v: &'static mut [u64] = vec![0u64; n].leak();
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const AtomicU64, n) }
}

// Steps over the decimal digits at d[*p..] and returns their value.
fn num(d: &[u8], p: &mut usize) -> u64 {
    let mut x = 0;
    while d[*p].is_ascii_digit() {
        x = x * 10 + (d[*p] - b'0') as u64;
        *p += 1;
    }
    x
}

// Parses one text field at d[*p..] and steps over it and the comma after it: (raw start, raw length,
// number of "" pairs). For a quoted field the raw bytes are those between the delimiting quotes.
fn field(d: &[u8], p: &mut usize) -> (usize, usize, usize) {
    let (s, mut q, mut pairs) = (*p + 1, *p + 1, 0);
    if d[*p] != b'"' {
        let n = d[*p..].iter().position(|&c| c == b',').unwrap();
        *p += n + 1;
        return (*p - n - 1, n, 0);
    }
    loop {
        q += d[q..].iter().position(|&c| c == b'"').unwrap();
        if d[q + 1] != b'"' {
            *p = q + 2;
            return (s, q - s, pairs);
        }
        (pairs, q) = (pairs + 1, q + 2);
    }
}

// Dense id of the raw name d[s..s+n], shared by all threads (key: offset << 16 | length, 0 = empty slot).
fn name_id(tab: &[AtomicU64], ids: &[AtomicU32], next: &AtomicU32, d: &[u8], s: usize, n: usize) -> u32 {
    let key = &d[s..s + n];
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &c in key {
        h = (h ^ c as u64).wrapping_mul(0x100_0000_01b3);
    }
    let mut i = (h >> 40) as usize & (SLOTS - 1);
    loop {
        let mut v = tab[i].load(Relaxed);
        if v == 0 {
            match tab[i].compare_exchange(0, (s as u64) << 16 | n as u64 | 1 << 63, Relaxed, Relaxed) {
                Ok(_) => {
                    let id = next.fetch_add(1, Relaxed) + 1;
                    ids[i].store(id, Relaxed);
                    return id;
                }
                Err(w) => v = w,
            }
        }
        let (o, l) = ((v >> 16 & (1 << 47) - 1) as usize, (v & 0xFFFF) as usize);
        if l == n && d[o..o + l] == *key {
            loop {
                let id = ids[i].load(Relaxed);
                if id != 0 {
                    return id; // the inserting thread may not have stored it yet
                }
                std::hint::spin_loop();
            }
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
    // regions.csv: region id -> country index + 1, countries in byte order.
    let r = fs::read(format!("{din}/regions.csv")).unwrap();
    let mut rows = vec![];
    let mut p = r.iter().position(|&c| c == b'\n').unwrap() + 1;
    while p < r.len() {
        let id = num(&r, &mut p) as usize;
        rows.push((id, [r[p + 1], r[p + 2], r[p + 3]]));
        p += 5;
    }
    let mut codes: Vec<[u8; 3]> = rows.iter().map(|x| x.1).collect();
    (codes.sort_unstable(), codes.dedup());
    let nc = codes.len(); // index nc is _UNMATCHED
    let mut country = vec![nc as u16; rows.iter().map(|x| x.0).max().unwrap_or(0) + 1];
    for (id, c) in &rows {
        country[*id] = codes.binary_search(c).unwrap() as u16;
    }
    let d = load(&format!("{din}/records.csv"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    // A '"' never occurs outside a quoted field, so "inside quotes" at p is the parity of the quotes
    // before p. Each chunk starts after the first LF outside quotes at or after its nominal start.
    let nom: Vec<usize> = (0..=nt).map(|k| k * d.len() / nt).collect();
    let quotes: Vec<usize> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|k| {
                let c = &d[nom[k]..nom[k + 1]];
                sc.spawn(move || c.iter().filter(|&&b| b == b'"').count())
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut bs = vec![d.iter().position(|&c| c == b'\n').map_or(d.len(), |x| x + 1)]; // after the header
    let mut inside = 0;
    for k in 1..nt {
        inside ^= quotes[k - 1] & 1;
        let (mut q, mut ins) = (nom[k], inside);
        while q < d.len() && (d[q] != b'\n' || ins == 1) {
            ins ^= (d[q] == b'"') as usize;
            q += 1;
        }
        bs.push((q + 1).min(d.len()).max(bs[k - 1]));
    }
    bs.push(d.len());
    let tab = zeroed(SLOTS);
    let ids: Vec<AtomicU32> = (0..SLOTS).map(|_| AtomicU32::new(0)).collect();
    let next = AtomicU32::new(0);
    let seen = zeroed((nc + 1) * IDS / 64);
    let (bs, country, ids, next) = (&bs, &country, &ids, &next);
    let parts: Vec<Vec<Agg>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|k| {
                sc.spawn(move || {
                    let mut agg = vec![ZERO; nc + 1];
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
                        let g = *country.get(reg).unwrap_or(&(nc as u16)) as usize;
                        let a = &mut agg[g];
                        (a.count += 1, a.ids += id, a.sum += amt);
                        (a.min = a.min.min(amt), a.max = a.max.max(amt));
                        (a.name_bytes += (nn - np) as i64, a.note_bytes += (tn - tp) as i64);
                        let nid = if nn == 0 { 0 } else { name_id(tab, ids, next, d, ns, nn) } as usize;
                        let w = &seen[g * IDS / 64 + nid / 64];
                        if w.load(Relaxed) & 1 << (nid % 64) == 0 {
                            w.fetch_or(1 << (nid % 64), Relaxed);
                        }
                    }
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!((next.load(Relaxed) as usize) < IDS);
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
        let names: u32 = seen[g * IDS / 64..(g + 1) * IDS / 64].iter().map(|w| w.load(Relaxed).count_ones()).sum();
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

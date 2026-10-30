// Speed Demon 2026, track A (Ingest). Spec: specs/a-ingest.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::{env, fs, thread};

const SLOTS: usize = 4096; // endpoint hash table; the catalogue has 1,842 templates
const HOURS: usize = 72; // the span is fixed by the spec: 2026-10-30T18 .. 2026-11-02T17

// Per line: low 16 bits of the latency; slot | 5xx << 12 | big << 13; high 16 bits if big.
type Hour = (Vec<u16>, Vec<u16>, Vec<u16>);

extern "C" {
    fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
}

fn load(path: &str) -> &'static [u8] {
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
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

// High bit set in each byte of w that equals c.
fn eq_bytes(w: u64, c: u64) -> u64 {
    let x = w ^ c * 0x0101_0101_0101_0101;
    x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080
}

// Up to eight ASCII digits, most significant in the low byte, zero bytes as leading zeros.
fn digits(x: u64) -> u32 {
    let x = x & 0x0F0F_0F0F_0F0F_0F0F;
    let x = (x.wrapping_mul(10) + (x >> 8)) & 0x00FF_00FF_00FF_00FF;
    let x = (x.wrapping_mul(100) + (x >> 16)) & 0x0000_FFFF_0000_FFFF;
    (x.wrapping_mul(10000) + (x >> 32)) as u32
}

// Finds or inserts the endpoint d[s..s+n] in the shared table (key: offset << 8 | length).
fn slot(tab: &[AtomicU64], d: &[u8], s: usize, n: usize, h: u64) -> usize {
    let mut i = (h >> 52) as usize;
    loop {
        let v = tab[i].load(Relaxed);
        let v = match v {
            0 => match tab[i].compare_exchange(0, (s as u64) << 8 | n as u64, Relaxed, Relaxed) {
                Ok(_) => return i,
                Err(w) => w,
            },
            _ => v,
        };
        if v & 255 == n as u64 && d[(v >> 8) as usize..][..n] == d[s..s + n] {
            return i;
        }
        i = (i + 1) & (SLOTS - 1);
    }
}

fn put(b: &mut Vec<u8>, x: usize) {
    if x >= 10 {
        put(b, x / 10);
    }
    b.push(b'0' + (x % 10) as u8);
}

// Parses the lines in d[a..b] into per-hour records.
fn parse(d: &[u8], a: usize, b: usize, tab: &[AtomicU64]) -> Vec<Hour> {
    let v = || Vec::with_capacity((b - a) / 40 + 1); // a line has >= 40 bytes: no vector grows
    let mut out: Vec<Hour> = (0..HOURS).map(|_| (v(), v(), v())).collect();
    let mut i = a;
    while i < b {
        let num = |k: usize| (d[i + k] - b'0') as usize * 10 + (d[i + k + 1] - b'0') as usize;
        let hour = (num(8) + 31 * (d[i + 6] - b'0') as usize) * 24 + num(11) - 738;
        let m = (d[i + 28] as usize * 7 + d[i + 29] as usize) >> 2 & 7;
        let s = i + 29 + [6, 3, 0, 0, 5, 3, 7, 4][m]; // method length from its first two bytes
        let (mut p, mut h) = (s, 0u64);
        loop {
            let w = word(d, p);
            let z = eq_bytes(w, b' ' as u64);
            if z != 0 {
                let k = z.trailing_zeros() as usize / 8;
                h = (h ^ (w & ((1u64 << (8 * k)) - 1)) ^ (p + k - s) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                p += k;
                break;
            }
            h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            p += 8;
        }
        let sl = slot(tab, d, s, p - s, h) as u16 | ((d[p + 1] == b'5') as u16) << 12;
        let w = word(d, p + 5);
        let k = eq_bytes(w, b'\n' as u64).trailing_zeros() as usize / 8;
        let mut v = digits(w << (64 - 8 * k)); // up to 8 digits at once; latency has 1 to 10
        p += 5 + k;
        while k == 8 && d[p] != b'\n' {
            v = v * 10 + (d[p] - b'0') as u32;
            p += 1;
        }
        let o = &mut out[hour];
        o.0.push(v as u16);
        o.1.push(sl | ((v > 0xFFFF) as u16) << 13);
        if v > 0xFFFF {
            o.2.push((v >> 16) as u16);
        }
        i = p + 1;
    }
    out
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    let d = load(&format!("{din}/requests.log"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let mut bs: Vec<usize> = (0..nt).map(|k| k * d.len() / nt).collect();
    for b in &mut bs[1..] {
        while *b > 0 && *b < d.len() && d[*b - 1] != b'\n' {
            *b += 1; // to a line start
        }
    }
    bs.push(d.len());
    let tab: Vec<AtomicU64> = (0..SLOTS).map(|_| AtomicU64::new(0)).collect();
    let (bs, tb) = (&bs, &tab);
    let parts: Vec<Vec<Hour>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn(move || parse(d, bs[t], bs[t + 1], tb))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut hours: Vec<Mutex<Vec<Hour>>> = (0..HOURS).map(|_| Mutex::new(Vec::new())).collect();
    for (h, x) in parts.into_iter().flat_map(|p| p.into_iter().enumerate()) {
        hours[h].get_mut().unwrap().push(x);
    }
    let key = |s: usize| tab[s].load(Relaxed);
    let ep = |s: usize| &d[(key(s) >> 8) as usize..][..(key(s) & 255) as usize];
    let mut order: Vec<usize> = (0..SLOTS).filter(|&s| key(s) != 0).collect();
    order.sort_by_key(|&s| ep(s));
    fs::create_dir_all(&dout).unwrap();
    let f = fs::File::create(format!("{dout}/summary.txt")).unwrap();
    // Workers take hours in order; whoever holds `ready` writes every finished buffer that is next.
    let next = AtomicUsize::new(0);
    let ready = Mutex::new((0, vec![None; HOURS]));
    // One hour per step: counting sort of its records by slot, then three percentiles per group.
    let work = || {
        let (mut cnt, mut errs, mut lat) = (vec![0u32; SLOTS + 1], vec![0usize; SLOTS], vec![]);
        while let h @ 0..HOURS = next.fetch_add(1, Relaxed) {
            let recs = std::mem::take(&mut *hours[h].lock().unwrap());
            (cnt.fill(0), errs.fill(0));
            for &m in recs.iter().flat_map(|r| &r.1) {
                cnt[(m & 4095) as usize + 1] += 1;
                errs[(m & 4095) as usize] += (m >> 12 & 1) as usize;
            }
            for s in 0..SLOTS {
                cnt[s + 1] += cnt[s];
            }
            let mut cur = cnt.clone();
            lat.resize(cnt[SLOTS] as usize, 0);
            for r in &recs {
                let mut hi = r.2.iter();
                for (&lo, &m) in r.0.iter().zip(&r.1) {
                    let v = lo as u32 | if m >> 13 != 0 { (*hi.next().unwrap() as u32) << 16 } else { 0 };
                    let g = (m & 4095) as usize;
                    lat[cur[g] as usize] = v;
                    cur[g] += 1;
                }
            }
            drop(recs);
            let ah = 30 * 24 + 18 + h; // hours since 2026-10-00T00
            let nov = (ah / 24 > 31) as usize;
            let day = ah / 24 - 31 * nov;
            let label = format!("2026-{}-{day:02}T{:02}:00:00Z ", 10 + nov, ah % 24);
            let mut buf = Vec::new();
            for &s in &order {
                let (a, b) = (cnt[s] as usize, cnt[s + 1] as usize);
                let (v, n) = (&mut lat[a..b], b - a);
                if n == 0 {
                    continue;
                }
                let r = [50, 95, 99].map(|p| (p * n + 99) / 100 - 1);
                if n <= 32 {
                    v.sort_unstable();
                } else {
                    v.select_nth_unstable(r[2]);
                    v[..r[2]].select_nth_unstable(r[1]);
                    v[..r[1]].select_nth_unstable(r[0]);
                }
                buf.extend_from_slice(label.as_bytes());
                buf.extend_from_slice(ep(s));
                let (e, q) = (errs[s], r.map(|k| v[k] as usize));
                for x in [n, q[0], q[1], q[2], e, e * 1_000_000 / n] {
                    let (mut t, mut i, mut x) = ([b' '; 21], 21, x);
                    loop {
                        i -= 1;
                        t[i] = b'0' + (x % 10) as u8;
                        x /= 10;
                        if x == 0 {
                            break;
                        }
                    }
                    buf.extend_from_slice(&t[i - 1..]);
                }
                buf.push(b'\n');
            }
            let mut g = ready.lock().unwrap();
            let (done, bufs) = &mut *g;
            bufs[h] = Some(buf);
            while let Some(b) = bufs.get_mut(*done).and_then(Option::take) {
                (&f).write_all(&b).unwrap();
                *done += 1;
            }
        }
    };
    thread::scope(|sc| (0..nt).for_each(|_| drop(sc.spawn(&work))));
}

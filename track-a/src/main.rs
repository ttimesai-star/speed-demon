// Speed Demon 2026, track A (Ingest). Spec: specs/a-ingest.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::*};
use std::sync::Mutex;
use std::{env, fs, thread};

const SLOTS: usize = 4096; // endpoint hash table; the catalogue has 1,842 templates
const HOURS: usize = 72; // the span is fixed by the spec: 2026-10-30T18 .. 2026-11-02T17
// Per line: low 16 bits of the latency, slot | 5xx << 12 | big << 13; high 16 bits only for big latencies.
type Hour = (Vec<u16>, Vec<u16>, Vec<u16>);

#[cfg(unix)]
fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    extern "C" {
        fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    }
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}

#[cfg(not(unix))]
fn load(path: &str) -> &'static [u8] {
    Box::leak(fs::read(path).unwrap().into_boxed_slice())
}

fn word(d: &[u8], p: usize) -> u64 {
    match d.get(p..p + 8) {
        Some(b) => u64::from_le_bytes(b.try_into().unwrap()),
        None => {
            let mut b = [0u8; 8];
            b[..d.len() - p].copy_from_slice(&d[p..]);
            u64::from_le_bytes(b)
        }
    }
}

// Up to eight ASCII digits, most significant in the low byte, zero bytes as leading zeros.
fn digits(x: u64) -> u32 {
    let x = x & 0x0F0F_0F0F_0F0F_0F0F;
    let x = (x.wrapping_mul(10) + (x >> 8)) & 0x00FF_00FF_00FF_00FF;
    let x = (x.wrapping_mul(100) + (x >> 16)) & 0x0000_FFFF_0000_FFFF;
    (x.wrapping_mul(10000) + (x >> 32)) as u32
}

// Finds or inserts the endpoint d[s..s+n] in the shared table; returns its slot.
fn slot(tab: &[AtomicU64], d: &[u8], s: usize, n: usize, h: u64) -> usize {
    let key = (s as u64) << 8 | n as u64;
    let mut i = (h >> 52) as usize;
    loop {
        let mut v = tab[i].load(Relaxed);
        if v == 0 {
            match tab[i].compare_exchange(0, key, Relaxed, Relaxed) {
                Ok(_) => return i,
                Err(w) => v = w,
            }
        }
        let (vs, vn) = ((v >> 8) as usize, (v & 255) as usize);
        if vn == n && d[vs..vs + n] == d[s..s + n] {
            return i;
        }
        i = (i + 1) & (SLOTS - 1);
    }
}

// Parses the lines in d[a..b] into one (latencies, slots) pair per hour.
fn parse(d: &[u8], a: usize, b: usize, tab: &[AtomicU64]) -> Vec<Hour> {
    let cap = (b - a) / 40 + 1; // a line has at least 40 bytes, so no vector ever grows
    let mut out: Vec<Hour> = (0..HOURS).map(|_| (Vec::with_capacity(cap), Vec::with_capacity(cap), Vec::with_capacity(cap))).collect();
    let mut i = a;
    while i < b {
        let num = |k: usize| (d[i + k] - b'0') as usize * 10 + (d[i + k + 1] - b'0') as usize;
        let hour = (num(8) + 31 * (d[i + 6] - b'0') as usize) * 24 + num(11) - 738;
        let m = (d[i + 28] as usize * 7 + d[i + 29] as usize) >> 2 & 7;
        let s = i + 29 + [6, 3, 0, 0, 5, 3, 7, 4][m]; // method length from its first two bytes
        let (mut p, mut h) = (s, 0u64);
        loop {
            let w = word(d, p);
            let x = w ^ 0x2020_2020_2020_2020;
            let z = x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080;
            if z != 0 {
                let k = z.trailing_zeros() as usize / 8;
                h = (h ^ (w & ((1u64 << (8 * k)) - 1)) ^ (p + k - s) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                p += k;
                break;
            }
            h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            p += 8;
        }
        let sl = (h >> 52) as u16 | ((d[p + 1] == b'5') as u16) << 12;
        p += 5;
        let w = word(d, p);
        let x = w ^ 0x0A0A_0A0A_0A0A_0A0A;
        let k = (x.wrapping_sub(0x0101_0101_0101_0101) & !x & 0x8080_8080_8080_8080).trailing_zeros() as usize / 8;
        let mut v = digits(w << (64 - 8 * k)); // up to 8 digits at once; latency has 1 to 10
        p += k;
        if k == 8 && d[p] != b'\n' {
            while d[p] != b'\n' {
                v = v * 10 + (d[p] - b'0') as u32;
                p += 1;
            }
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
    let args: Vec<String> = env::args().collect();
    let (din, dout) = (args.get(1).map_or("/data", |s| s), args.get(2).map_or("/out", |s| s));
    let d = load(&format!("{din}/requests.log"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let cut = |k: usize| match k * d.len() / nt {
        x if k == nt => x,
        x => d[x..].iter().position(|&c| c == b'\n').map_or(d.len(), |p| x + p + 1) * (k > 0) as usize,
    };
    let bounds: Vec<usize> = (0..=nt).map(cut).collect();
    let tab: Vec<AtomicU64> = (0..SLOTS).map(|_| AtomicU64::new(0)).collect();
    let (bs, tb) = (&bounds, &tab);
    let parts: Vec<Vec<Hour>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt).map(|t| sc.spawn(move || parse(d, bs[t], bs[t + 1], tb))).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut hours: Vec<Mutex<Vec<Hour>>> = (0..HOURS).map(|_| Mutex::new(Vec::new())).collect();
    for p in parts {
        for (h, x) in p.into_iter().enumerate() {
            hours[h].get_mut().unwrap().push(x);
        }
    }
    let ep = |s: usize| {
        let v = tab[s].load(Relaxed);
        &d[(v >> 8) as usize..(v >> 8) as usize + (v & 255) as usize]
    };
    let mut order: Vec<usize> = (0..SLOTS).filter(|&s| tab[s].load(Relaxed) != 0).collect();
    order.sort_by_key(|&s| ep(s));
    fs::create_dir_all(dout).unwrap();
    let f = fs::File::create(format!("{dout}/summary.txt")).unwrap();
    let next = AtomicUsize::new(0);
    let ready = Mutex::new((0, vec![None; HOURS])); // next hour to write, finished buffers
    thread::scope(|sc| {
        for _ in 0..nt {
            sc.spawn(|| {
                let (mut cnt, mut errs, mut cur) = (vec![0u32; SLOTS + 1], vec![0u32; SLOTS], vec![0u32; SLOTS]);
                let mut lat = Vec::new();
                loop {
                    let h = next.fetch_add(1, Relaxed);
                    if h >= HOURS {
                        break;
                    }
                    let recs = std::mem::take(&mut *hours[h].lock().unwrap());
                    let mut buf = Vec::new();
                    (cnt.fill(0), errs.fill(0));
                    for &m in recs.iter().flat_map(|r| &r.1) {
                        cnt[(m & 4095) as usize + 1] += 1;
                        errs[(m & 4095) as usize] += (m >> 12 & 1) as u32;
                    }
                    for s in 0..SLOTS {
                        cnt[s + 1] += cnt[s];
                    }
                    cur.copy_from_slice(&cnt[..SLOTS]);
                    lat.resize(cnt[SLOTS] as usize, 0);
                    for r in &recs {
                        let mut hi = r.2.iter();
                        for (&lo, &m) in r.0.iter().zip(&r.1) {
                            let v = lo as u32 | if m >> 13 != 0 { (*hi.next().unwrap() as u32) << 16 } else { 0 };
                            lat[cur[(m & 4095) as usize] as usize] = v;
                            cur[(m & 4095) as usize] += 1;
                        }
                    }
                    drop(recs);
                    let ah = 30 * 24 + 18 + h; // hours since 2026-10-00T00
                    let (day, mon) = if ah / 24 > 31 { (ah / 24 - 31, 11) } else { (ah / 24, 10) };
                    let label = format!("2026-{mon}-{day:02}T{:02}:00:00Z ", ah % 24);
                    for &s in &order {
                        let (a, b) = (cnt[s] as usize, cnt[s + 1] as usize);
                        if a == b {
                            continue;
                        }
                        let (v, n) = (&mut lat[a..b], b - a);
                        let r = [(50 * n + 99) / 100 - 1, (95 * n + 99) / 100 - 1, (99 * n + 99) / 100 - 1];
                        if n <= 32 {
                            v.sort_unstable();
                        } else {
                            v.select_nth_unstable(r[2]);
                            v[..r[2]].select_nth_unstable(r[1]);
                            v[..r[1]].select_nth_unstable(r[0]);
                        }
                        let e = errs[s] as u64;
                        buf.extend_from_slice(label.as_bytes());
                        buf.extend_from_slice(ep(s));
                        for x in [n as u64, v[r[0]] as u64, v[r[1]] as u64, v[r[2]] as u64, e, e * 1_000_000 / n as u64] {
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
            });
        }
    });
}

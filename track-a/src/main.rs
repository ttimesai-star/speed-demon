// Speed Demon 2026, track A (Ingest). Spec: specs/a-ingest.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::{env, fs, thread};

const SLOTS: usize = 4096; // endpoint hash table; the catalogue has 1,842 templates
const HOURS: usize = 72; // the span is fixed by the spec: 2026-10-30T18 .. 2026-11-02T17

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

// Finds or inserts the endpoint d[s..s+n] in the shared table; returns its slot.
fn slot(tab: &[AtomicU64], d: &[u8], s: usize, n: usize, h: u32) -> usize {
    let key = (s as u64) << 8 | n as u64;
    let mut i = h as usize & (SLOTS - 1);
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

// Parses the lines in d[a..b]; calls f(group, is_5xx, latency) for each.
fn walk(d: &[u8], a: usize, b: usize, tab: &[AtomicU64], hsrc: &[AtomicUsize], mut f: impl FnMut(usize, bool, u32)) {
    let mut i = a;
    while i < b {
        let num = |k: usize| (d[i + k] - b'0') as usize * 10 + (d[i + k + 1] - b'0') as usize;
        let hour = (num(8) + 31 * (d[i + 6] - b'0') as usize) * 24 + num(11) - 738;
        hsrc[hour].store(i, Relaxed);
        let mut p = i + 31;
        while d[p] != b' ' {
            p += 1;
        }
        let s = p + 1;
        let mut h = 0u32;
        p = s;
        while d[p] != b' ' {
            h = h.wrapping_mul(31).wrapping_add(d[p] as u32);
            p += 1;
        }
        let g = hour * SLOTS + slot(tab, d, s, p - s, h);
        let err = d[p + 1] == b'5';
        p += 5;
        let mut v = 0u32;
        while d[p] != b'\n' {
            v = v * 10 + (d[p] - b'0') as u32;
            p += 1;
        }
        f(g, err, v);
        i = p + 1;
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let (din, dout) = (args.get(1).map_or("/data", |s| s), args.get(2).map_or("/out", |s| s));
    let d = load(&format!("{din}/requests.log"));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    let cut = |k: usize| match k {
        0 => 0,
        _ if k >= nt => d.len(),
        _ => d[k * d.len() / nt..].iter().position(|&c| c == b'\n').unwrap() + k * d.len() / nt + 1,
    };
    let bounds: Vec<usize> = (0..=nt).map(cut).collect();
    let tab: Vec<AtomicU64> = (0..SLOTS).map(|_| AtomicU64::new(0)).collect();
    let hsrc: Vec<AtomicUsize> = (0..HOURS).map(|_| AtomicUsize::new(0)).collect();
    let cnt: Vec<AtomicU32> = (0..HOURS * SLOTS).map(|_| AtomicU32::new(0)).collect();
    let errs: Vec<AtomicU32> = (0..HOURS * SLOTS).map(|_| AtomicU32::new(0)).collect();
    let bounds = &bounds;
    let par = |f: &(dyn Fn(usize, usize) + Sync)| {
        thread::scope(|sc| {
            for t in 0..nt {
                sc.spawn(move || f(bounds[t], bounds[t + 1]));
            }
        })
    };
    par(&|a, b| {
        walk(d, a, b, &tab, &hsrc, |g, e, _| {
            cnt[g].fetch_add(1, Relaxed);
            if e {
                errs[g].fetch_add(1, Relaxed);
            }
        })
    });
    let mut off = vec![0u32; HOURS * SLOTS + 1];
    for g in 0..HOURS * SLOTS {
        off[g + 1] = off[g] + cnt[g].swap(off[g], Relaxed);
    }
    let mut lat = vec![0u32; off[HOURS * SLOTS] as usize];
    let la = unsafe { &*(lat.as_mut_slice() as *mut [u32] as *const [AtomicU32]) };
    par(&|a, b| walk(d, a, b, &tab, &hsrc, |g, _, v| la[cnt[g].fetch_add(1, Relaxed) as usize].store(v, Relaxed)));

    let ep = |v: u64| &d[(v >> 8) as usize..(v >> 8) as usize + (v & 255) as usize];
    let mut order: Vec<usize> = (0..SLOTS).filter(|&s| tab[s].load(Relaxed) != 0).collect();
    order.sort_by_key(|&s| ep(tab[s].load(Relaxed)));
    let mut work = Vec::new();
    let mut rest = lat.as_mut_slice();
    for h in 0..HOURS {
        let n = (off[(h + 1) * SLOTS] - off[h * SLOTS]) as usize;
        let (part, tail) = rest.split_at_mut(n);
        work.push((h, part));
        rest = tail;
    }
    let work = Mutex::new(work);
    let mut outs: Vec<Mutex<Vec<u8>>> = (0..HOURS).map(|_| Mutex::new(Vec::new())).collect();
    thread::scope(|sc| {
        for _ in 0..nt {
            sc.spawn(|| loop {
                let Some((h, part)) = work.lock().unwrap().pop() else { break };
                let mut buf = Vec::new();
                let base = off[h * SLOTS];
                let hs = hsrc[h].load(Relaxed);
                for &s in &order {
                    let g = h * SLOTS + s;
                    let (a, b) = ((off[g] - base) as usize, (cnt[g].load(Relaxed) - base) as usize);
                    if a == b {
                        continue;
                    }
                    let v = &mut part[a..b];
                    let n = b - a;
                    let r = [(50 * n + 99) / 100 - 1, (95 * n + 99) / 100 - 1, (99 * n + 99) / 100 - 1];
                    v.select_nth_unstable(r[2]);
                    if r[1] < r[2] {
                        v[..r[2]].select_nth_unstable(r[1]);
                    }
                    if r[0] < r[1] {
                        v[..r[1]].select_nth_unstable(r[0]);
                    }
                    let e = errs[g].load(Relaxed) as u64;
                    buf.extend_from_slice(&d[hs..hs + 13]);
                    buf.extend_from_slice(b":00:00Z ");
                    buf.extend_from_slice(ep(tab[s].load(Relaxed)));
                    writeln!(buf, " {n} {} {} {} {e} {}", v[r[0]], v[r[1]], v[r[2]], e * 1_000_000 / n as u64).unwrap();
                }
                *outs[h].lock().unwrap() = buf;
            });
        }
    });
    fs::create_dir_all(dout).unwrap();
    let mut f = fs::File::create(format!("{dout}/summary.txt")).unwrap();
    for o in outs.iter_mut() {
        f.write_all(o.get_mut().unwrap()).unwrap();
    }
}

// Speed Demon 2026, track C (Join). Spec: specs/c-join.md in hackathon-raptors/speed-demon-2026.
use std::arch::x86_64::*;
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::{Acquire, Relaxed, Release}};
use std::{env, fs, thread};

extern "C" {
    fn mmap(a: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn madvise(a: *const u8, len: usize, advice: i32) -> i32;
    fn prctl(op: i32, ...) -> i32;
}

fn load(path: &str) -> &'static [u8] {
    use std::os::unix::io::AsRawFd;
    unsafe { prctl(41, 1u64, 0u64, 0u64, 0u64) }; // PR_SET_THP_DISABLE: sparse bitsets stay sparse
    let f = fs::File::open(path).unwrap();
    let n = f.metadata().unwrap().len() as usize;
    if n == 0 {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(mmap(std::ptr::null_mut(), n, 1, 2, f.as_raw_fd(), 0), n) }
}

const SLOTS: usize = 1 << 17; // name table; the generator's pool has 100,000 names (ids < 2^20)
const IDS: usize = 1 << 20; // bits per country bitset, only the touched pages cost memory
static TOP: AtomicU64 = AtomicU64::new(0); // arena words << 20 | names

// Zeroed atomics from calloc'd memory, so untouched pages are never faulted in.
fn zeroed<T>(n: usize) -> &'static [T] {
    let v = vec![0u64; (n * std::mem::size_of::<T>()).div_ceil(8)].leak();
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const T, n) }
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

// Bit i of each mask is set where byte p + i of d is a quote, a comma, an LF (zero-padded past the end).
fn masks(d: &[u8], p: usize) -> [u64; 3] {
    let (mut pad, n) = ([0u8; 64], (d.len() - p).min(64));
    if n < 64 {
        pad[..n].copy_from_slice(&d[p..]);
    }
    let s = [pad.as_ptr(), d[p..].as_ptr()][(n == 64) as usize];
    [b'"', b',', b'\n'].map(|c| unsafe {
        let v = _mm256_set1_epi8(c as i8);
        let m = |o| _mm256_movemask_epi8(_mm256_cmpeq_epi8(_mm256_loadu_si256(s.add(o) as _), v));
        m(0) as u32 as u64 | (m(32) as u32 as u64) << 32
    })
}

// Word k of the raw name d[s..s+n], the bytes past n zeroed (0 past the end of the name).
fn nw(d: &[u8], s: usize, n: usize, k: usize) -> u64 {
    let r = n.saturating_sub(8 * k).min(8);
    word(d, s + 8 * k) & u64::MAX >> (64 - 8 * r).min(63) & 0u64.wrapping_sub((r > 0) as u64)
}

// First slot of the raw name d[s..s+n] in the name table: a hash of its length and first 16 bytes, no loop.
fn name_slot(d: &[u8], s: usize, n: usize) -> usize {
    let h = (nw(d, s, n, 0) ^ n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    (h ^ nw(d, s, n, 1)).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) as usize >> 40 & (SLOTS - 1)
}

// Dense id of the raw name d[s..s+n] whose first slot is i (see name_slot), shared by all threads. Slot i is
// t[4i..4i+3]: arena index << 36 | length << 20 | id (bit 63 set, 0 = empty, 1 = being filled), then the
// name's first 16 bytes, so nine lookups in ten read one cache line; a longer name's further words are in the
// arena after the table.
fn name_id(t: &[AtomicU64], d: &[u8], s: usize, n: usize, mut i: usize) -> u64 {
    let w = |k| nw(d, s, n, k);
    loop {
        let mut v = t[4 * i].load(Acquire);
        if v == 0 && t[4 * i].compare_exchange(0, 1, Acquire, Relaxed).is_ok() {
            let words = n.div_ceil(8).saturating_sub(2);
            let o = TOP.fetch_add((words as u64) << 20 | 1, Relaxed);
            let (at, id) = (4 * SLOTS + (o >> 20) as usize, (o & 0xFFFFF) + 1);
            for k in 0..words {
                t[at + k].store(w(k + 2), Relaxed);
            }
            (t[4 * i + 1].store(w(0), Relaxed), t[4 * i + 2].store(w(1), Relaxed));
            t[4 * i].store(1 << 63 | (at as u64) << 36 | (n as u64) << 20 | id, Release);
            return id;
        }
        while v <= 1 {
            v = t[4 * i].load(Acquire); // another thread is filling this slot
        }
        let (at, l) = ((v >> 36 & 0x7FF_FFFF) as usize, (v >> 20 & 0xFFFF) as usize);
        let eq = |k: usize, j: usize| t[j].load(Relaxed) == w(k);
        let long = |k: usize| eq(k, at + k - 2);
        if (l == n) & eq(0, 4 * i + 1) & eq(1, 4 * i + 2) && (2..n.div_ceil(8)).all(long) {
            return v & 0xFFFFF;
        }
        i = (i + 1) & (SLOTS - 1);
    }
}

// Per country: count, id sum, amount sum, min, max, name bytes, note bytes.
const ZERO: [i64; 7] = [0, 0, 0, i64::MAX, i64::MIN, 0, 0];

fn merge(a: &mut [i64; 7], v: [i64; 7]) {
    for j in 0..7 {
        let k = (j == 3) as usize + 2 * (j == 4) as usize;
        a[j] = [a[j] + v[j], a[j].min(v[j]), a[j].max(v[j])][k];
    }
}

fn money(out: &mut Vec<u8>, c: i64) {
    let sign = if c < 0 { "-" } else { "" };
    write!(out, " {sign}{}.{:02}", c.abs() / 100, c.abs() % 100).unwrap();
}

fn pf<const W: i32, T>(p: &T) {
    unsafe { _mm_prefetch::<W>(p as *const T as *const i8) };
}

fn main() {
    let arg = |i: usize, def: &str| env::args().nth(i).unwrap_or(def.into());
    let (din, dout) = (arg(1, "/data"), arg(2, "/out"));
    let (r, d) = (load(&format!("{din}/regions.csv")), load(&format!("{din}/records.csv")));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    // Line starts after the header and near k / nt of each file. For records.csv without a pass over the file:
    // by section 2.2 a note is lowercase words (an LF in it is followed by a letter, a quote or a space-free
    // word) and a name has no LF, so an LF followed by a digit or by the first byte of a BOM ends a record.
    let starts = |d: &[u8], ok: fn(u8) -> bool| {
        let mut bs = vec![1 + d.iter().position(|&c| c == b'\n').unwrap()];
        for k in 1..nt {
            let from = (k * d.len() / nt).max(bs[k - 1]);
            let at = (from..d.len()).find(|&i| d[i - 1] == b'\n' && ok(d[i]));
            bs.push(at.unwrap_or(d.len()));
        }
        (bs.push(d.len()), bs).1
    };
    let (bs, rs) = (starts(d, |c| c.is_ascii_digit() || c == 0xEF), starts(r, |_| true));
    // regions.csv in parallel, by rows: first the codes present (a code is a base-26 number, so the order of
    // the numbers is the byte order), then country[id] = 1 + the rank of its code (0: no such region, and ids
    // are below 10^7). Country index 255 is _UNMATCHED.
    let rows = |k: usize, f: &mut dyn FnMut(usize, usize)| {
        let mut p = rs[k];
        while p < rs[k + 1] {
            let id = num(r, &mut p) as usize;
            f(id, r[p + 1..p + 4].iter().fold(0, |k, &c| k * 26 + (c - b'A') as usize));
            p += 5;
        }
    };
    let (has, country) = (zeroed::<AtomicU8>(26 * 26 * 26), zeroed::<AtomicU8>(10_000_000));
    let (t, seen) = (zeroed::<AtomicU64>(4 * SLOTS + (1 << 24)), zeroed::<AtomicU64>(256 * IDS / 64));
    let (bs, rows, wait) = (&bs, &rows, &std::sync::Barrier::new(nt));
    let parts: Vec<Vec<[i64; 7]>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|k| {
                sc.spawn(move || {
                    rows(k, &mut |_, key| has[key].store(1, Relaxed));
                    wait.wait();
                    let rank: Vec<u8> = has.iter().scan(1, |n, h| Some((*n, *n += h.load(Relaxed)).0)).collect();
                    rows(k, &mut |id, key| country[id].store(rank[key], Relaxed));
                    wait.wait();
                    let (mut agg, mut pend, mut bit) = (vec![ZERO; 256], None, None);
                    let set = |(k, b): (usize, u64)| {
                        if seen[k].load(Relaxed) & b == 0 {
                            seen[k].fetch_or(b, Relaxed);
                        }
                    };
                    // Stage 2 of 3: the name's id, then a prefetch for writing (prefetchw) of its word in the
                    // country's bitset; the bit itself is set one record later (stage 3).
                    let mut mark = |(ns, nn, i, g): (usize, usize, usize, usize)| {
                        let nid = if nn == 0 { 0 } else { name_id(t, d, ns, nn, i) as usize };
                        let k = g * IDS / 64 + nid / 64;
                        pf::<7, _>(&seen[k]);
                        bit.replace((k, 1u64 << (nid % 64))).map(set);
                    };
                    // One record from its field starts and ends and the quotes in each field: a quoted field
                    // of n bytes holds 2 + 2 * pairs quotes, so its value is n - 1 - quotes / 2 bytes.
                    let mut rec = |st: [usize; 5], en: [usize; 5], qs: [usize; 5]| {
                        let mut p = st[0] + 3 * (d[st[0]] == 0xEF) as usize;
                        let id = num(d, &mut p) as i64;
                        let (n, tn) = (en[1] - st[1], en[2] - st[2]);
                        let (ns, nn) = (st[1] + (n > 0) as usize, n.saturating_sub(2));
                        let bytes = [n.saturating_sub(1 + qs[1] / 2), tn.saturating_sub(1 + qs[2] / 2)];
                        p = st[3];
                        let reg = num(d, &mut p) as usize;
                        pf::<3, _>(&country[reg.min(country.len() - 1)]); // used after the amount is parsed
                        p = st[4] + (d[st[4]] == b'-') as usize;
                        let cents = num(d, &mut p) as i64 * 100 + (d[p + 1] - b'0') as i64 * 10;
                        let amt = (cents + (d[p + 2] - b'0') as i64) * [1, -1][(d[st[4]] == b'-') as usize];
                        let g = country.get(reg).map_or(0, |c| c.load(Relaxed)).wrapping_sub(1) as usize;
                        let v = [1, id, amt, amt, amt, bytes[0] as i64, bytes[1] as i64];
                        merge(&mut agg[g], v);
                        // Stage 1: this record's slot is prefetched, the previous record's name is looked up.
                        let i = name_slot(d, ns, nn);
                        pf::<3, _>(&t[4 * i]);
                        pend.replace((ns, nn, i, g)).map(&mut mark);
                    };
                    // Structure 64 bytes at a time: a byte is inside quotes when an odd number of quotes precede
                    // it in the record (a "" pair flips twice), so the separators are the commas and LFs outside
                    // quotes, four commas and then the LF per record. q counts the quotes of the field so far.
                    let (mut f, mut st, mut en, mut qs) = (0, [bs[k]; 5], [0; 5], [0; 5]);
                    let (mut blk, mut ins, mut q) = (bs[k], 0u64, 0);
                    while blk < bs[k + 1] {
                        let [qm, cm, lm] = masks(d, blk);
                        let mut inq = qm;
                        for s in [1, 2, 4, 8, 16, 32] {
                            inq ^= inq << s; // prefix XOR: set from an opening quote to the byte before its closing one
                        }
                        inq ^= ins;
                        ins = ((inq as i64) >> 63) as u64;
                        let mut sep = (cm | lm) & !inq & u64::MAX >> (64 - (bs[k + 1] - blk).min(64));
                        let mut left = qm;
                        while sep != 0 {
                            let b = sep.trailing_zeros() as usize;
                            let below = u64::MAX >> (63 - b); // bits 0..=b
                            (qs[f], en[f]) = (q + (left & below).count_ones() as usize, blk + b);
                            (left &= !below, sep &= sep - 1, q = 0, f += 1);
                            if f == 5 {
                                (rec(st, en, qs), f = 0);
                            }
                            st[f] = blk + b + 1;
                        }
                        q += left.count_ones() as usize;
                        blk += 64;
                    }
                    pend.map(&mut mark);
                    bit.map(set);
                    // MADV_DONTNEED on this thread's pages of the input: the threads unmap them in parallel
                    // instead of the process exit doing all of it in one thread.
                    let (a, e) = (bs[k].next_multiple_of(4096), bs[k + 1] / 4096 * 4096);
                    unsafe { madvise(d.as_ptr().add(a.min(e)), e - a.min(e), 4) };
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let top = TOP.load(Relaxed);
    assert!(((top & 0xFFFFF) as usize) < IDS && 4 * SLOTS + (top >> 20) as usize <= t.len());
    let codes: Vec<usize> = (0..has.len()).filter(|&k| has[k].load(Relaxed) == 1).collect();
    assert!(codes.len() < 255);
    let mut out = Vec::new();
    for (g, code) in codes.iter().map(|&k| Some(k)).enumerate().chain([(255, None)]) {
        let mut a = ZERO;
        parts.iter().for_each(|x| merge(&mut a, x[g]));
        if a[0] == 0 {
            continue;
        }
        let used = (top & 0xFFFFF) as usize / 64 + 1; // words that can hold a set bit
        let names: u32 = seen[g * IDS / 64..][..used].iter().map(|w| w.load(Relaxed).count_ones()).sum();
        match code {
            Some(k) => out.extend([k / 676, k / 26 % 26, k % 26].map(|x| b'A' + x as u8)),
            None => out.extend(b"_UNMATCHED"),
        }
        write!(out, " {} {}", a[0], a[1]).unwrap();
        for c in [a[2], a[3], a[4]] {
            money(&mut out, c);
        }
        writeln!(out, " {} {} {}", names, a[5], a[6]).unwrap();
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/aggregates.txt"), out).unwrap();
}

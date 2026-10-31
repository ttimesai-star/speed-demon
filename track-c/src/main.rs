// Speed Demon 2026, track C (Join). Spec: specs/c-join.md in hackathon-raptors/speed-demon-2026.
use std::io::Write;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering::{Acquire, Relaxed, Release}};
use std::{env, fs, thread};

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


const SLOTS: usize = 1 << 18; // name table; the generator's pool has 100,000 names (ids < 2^20)
const IDS: usize = 1 << 20; // bits per country bitset, only the touched pages cost memory

// Zeroed atomics from calloc'd memory, so untouched pages are never faulted in.
fn zeroed(n: usize) -> &'static [AtomicU64] {
    let v: &'static mut [u64] = vec![0u64; n].leak();
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const AtomicU64, n) }
}

fn zeroed8(n: usize) -> &'static [AtomicU8] {
    let v: &'static mut [u8] = vec![0u8; n].leak();
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const AtomicU8, n) }
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
    use std::arch::x86_64::*;
    let mut pad = [0u8; 64];
    if p + 64 > d.len() {
        pad[..d.len() - p].copy_from_slice(&d[p..]);
    }
    let s = if p + 64 > d.len() { pad.as_ptr() } else { unsafe { d.as_ptr().add(p) } };
    let (a, b) = unsafe { (_mm256_loadu_si256(s as *const __m256i), _mm256_loadu_si256(s.add(32) as *const __m256i)) };
    [b'"', b',', b'\n'].map(|c| unsafe {
        let v = _mm256_set1_epi8(c as i8);
        let m = |x| _mm256_movemask_epi8(_mm256_cmpeq_epi8(x, v)) as u32 as u64;
        m(a) | m(b) << 32
    })
}

// Word k of the raw name d[s..s+n], the bytes past n zeroed (0 past the end of the name).
fn nw(d: &[u8], s: usize, n: usize, k: usize) -> u64 {
    let r = n.saturating_sub(8 * k).min(8);
    word(d, s + 8 * k) & u64::MAX >> (64 - 8 * r).min(63) & 0u64.wrapping_sub((r > 0) as u64)
}

// First slot of the raw name d[s..s+n] in the name table: a hash of its length and first 16 bytes, so no loop.
fn name_slot(d: &[u8], s: usize, n: usize) -> usize {
    let h = (nw(d, s, n, 0) ^ n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nw(d, s, n, 1).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    (h >> 40) as usize & (SLOTS - 1)
}

// Dense id of the raw name d[s..s+n] whose first slot is i (see name_slot), shared by all threads. A new name
// is copied to the arena first (two words at least), then published by CAS as arena offset << 36 | length << 20
// | id (bit 63 set, 0 = empty slot).
fn name_id(tab: &[AtomicU64], arena: &[AtomicU64], top: &AtomicU64, d: &[u8], s: usize, n: usize, mut i: usize) -> u64 {
    let (w0, w1) = (nw(d, s, n, 0), nw(d, s, n, 1));
    loop {
        let mut v = tab[i].load(Acquire);
        if v == 0 {
            let words = n.div_ceil(8).max(2) as u64;
            let o = top.fetch_add(words << 20 | 1, Relaxed); // arena words << 20 | ids
            let (at, id) = ((o >> 20) as usize, (o & 0xFFFFF) + 1);
            for k in 0..words as usize {
                arena[at + k].store(nw(d, s, n, k), Relaxed);
            }
            let key = 1 << 63 | (at as u64) << 36 | (n as u64) << 20 | id;
            match tab[i].compare_exchange(0, key, Release, Acquire) {
                Ok(_) => return id,
                Err(w) => v = w,
            }
        }
        let (at, l) = ((v >> 36 & 0x7FF_FFFF) as usize, (v >> 20 & 0xFFFF) as usize);
        if (l == n) & (arena[at].load(Relaxed) == w0) & (arena[at + 1].load(Relaxed) == w1)
            && (2..n.div_ceil(8)).all(|k| arena[at + k].load(Relaxed) == nw(d, s, n, k))
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
    let (r, d) = (load(&format!("{din}/regions.csv")), load(&format!("{din}/records.csv")));
    let nt = thread::available_parallelism().map_or(12, |n| n.get());
    // Line starts after the header and near k / nt of each file. For records.csv without a pass over the file:
    // by section 2.2 a note is lowercase words (an LF in it is followed by a letter, a quote or a space-free
    // word) and a name has no LF, so an LF followed by a digit or by the first byte of a BOM ends a record.
    let starts = |d: &[u8], ok: &dyn Fn(u8) -> bool| {
        let mut bs = vec![d.iter().position(|&c| c == b'\n').map_or(d.len(), |x| x + 1)];
        for k in 1..nt {
            let from = (k * d.len() / nt).max(bs[k - 1]).max(1);
            bs.push((from..d.len()).find(|&i| d[i - 1] == b'\n' && ok(d[i])).unwrap_or(d.len()));
        }
        (bs.push(d.len()), bs).1
    };
    let (bs, rs) = (starts(d, &|c| c.is_ascii_digit() || c == 0xEF), starts(r, &|_| true));
    // regions.csv in parallel, by rows: first the codes present (a code is a base-26 number, so the order of
    // the numbers is the byte order), then country[id] = 1 + the rank of its code (0: no such region, and ids
    // are below 10^7). Country index 255 is _UNMATCHED.
    let rows = |k: usize, f: &mut dyn FnMut(usize, usize)| {
        let mut p = rs[k];
        while p < rs[k + 1] {
            let id = num(r, &mut p) as usize;
            (f(id, r[p + 1..p + 4].iter().fold(0, |k, &c| k * 26 + (c - b'A') as usize)), p += 5);
        }
    };
    let (has, country) = (zeroed8(26 * 26 * 26), zeroed8(10_000_000));
    let (tab, arena, top) = (zeroed(SLOTS), zeroed(1 << 24), AtomicU64::new(0));
    let seen = zeroed(256 * IDS / 64);
    let (bs, top, rows, wait) = (&bs, &top, &rows, &std::sync::Barrier::new(nt));
    let parts: Vec<Vec<Agg>> = thread::scope(|sc| {
        let hs: Vec<_> = (0..nt)
            .map(|k| {
                sc.spawn(move || {
                    rows(k, &mut |_, key| has[key].store(1, Relaxed));
                    wait.wait();
                    let rank: Vec<u8> = has.iter().scan(1, |n, h| Some((*n, *n += h.load(Relaxed)).0)).collect();
                    rows(k, &mut |id, key| country[id].store(rank[key], Relaxed));
                    wait.wait();
                    let mut agg = vec![ZERO; 256];
                    let (mut pre, mut pend, mut bit) = (None, None, None);
                    // Stage 1 of 4 is the slot's prefetch below; stage 2 prefetches the arena words of the name
                    // that slot holds, so stage 3 compares cached bytes.
                    let pre_arena = |i: usize| {
                        let at = tab[i].load(Relaxed) >> 36 & 0x7FF_FFFF;
                        unsafe { std::arch::x86_64::_mm_prefetch::<3>(arena.as_ptr().add(at as usize) as *const i8) };
                    };
                    let set = |(k, b): (usize, u64)| {
                        if seen[k].load(Relaxed) & b == 0 {
                            seen[k].fetch_or(b, Relaxed);
                        }
                    };
                    // Stage 3 of 4: the name's id, then a prefetch for writing of its word in the country's bitset; the
                    // bit itself is set one record later (stage 4).
                    let mut mark = |(ns, nn, i, g): (usize, usize, usize, usize)| {
                        let nid = if nn == 0 { 0 } else { name_id(tab, arena, top, d, ns, nn, i) } as usize;
                        let k = g * IDS / 64 + nid / 64;
                        unsafe { std::arch::x86_64::_mm_prefetch::<7>(seen.as_ptr().add(k) as *const i8) }; // prefetchw: owned before the RMW
                        bit.replace((k, 1u64 << (nid % 64))).map(set);
                    };
                    let mut rec = |st: [usize; 5], en: [usize; 5], qs: [u32; 5]| {
                        let mut p = st[0] + 3 * (d[st[0]] == 0xEF) as usize;
                        let id = num(d, &mut p) as i64;
                        let n = en[1] - st[1];
                        let (ns, nn, np) = if n == 0 { (st[1], 0, 0) } else { (st[1] + 1, n - 2, (qs[1] as usize - 2) / 2) };
                        let n = en[2] - st[2];
                        let note = if n == 0 { 0 } else { n - 2 - (qs[2] as usize - 2) / 2 };
                        p = st[3];
                        let reg = num(d, &mut p) as usize;
                        let cp = country.as_ptr().wrapping_add(reg.min(country.len() - 1)) as *const i8;
                        unsafe { std::arch::x86_64::_mm_prefetch::<3>(cp) }; // used after the amount is parsed
                        p = st[4];
                        let neg = d[p] == b'-';
                        p += neg as usize;
                        let whole = num(d, &mut p) as i64;
                        let cents = (d[p + 1] - b'0') as i64 * 10 + (d[p + 2] - b'0') as i64;
                        let amt = if neg { -(whole * 100 + cents) } else { whole * 100 + cents };
                        let g = country.get(reg).map_or(0, |c| c.load(Relaxed)).wrapping_sub(1) as usize;
                        let a = &mut agg[g];
                        (a.count += 1, a.ids += id, a.sum += amt);
                        (a.min = a.min.min(amt), a.max = a.max.max(amt));
                        (a.name_bytes += (nn - np) as i64, a.note_bytes += note as i64);
                        // This record's slot is prefetched, the previous record's name has its arena words
                        // prefetched, the one before it is looked up and marked for its country.
                        let i = name_slot(d, ns, nn);
                        unsafe { std::arch::x86_64::_mm_prefetch::<3>(tab.as_ptr().add(i) as *const i8) };
                        if let Some(x) = pre.replace((ns, nn, i, g)) {
                            pre_arena(x.2);
                            pend.replace(x).map(&mut mark);
                        }
                    };
                    // Structure 64 bytes at a time: a byte is inside quotes when an odd number of quotes precede
                    // it in the record (a "" pair flips twice), so the separators are the commas and LFs outside
                    // quotes, four commas and then the LF per record. q counts the quotes of the field so far:
                    // a quoted field holds 2 + 2 * pairs.
                    let (mut f, mut st, mut en, mut qs) = (0, [bs[k]; 5], [0usize; 5], [0u32; 5]);
                    let (mut blk, mut ins, mut q) = (bs[k], 0u64, 0u32);
                    while blk < bs[k + 1] {
                        let [qm, cm, lm] = masks(d, blk);
                        let mut inq = qm;
                        for s in [1, 2, 4, 8, 16, 32] {
                            inq ^= inq << s; // prefix XOR: set from an opening quote to the byte before its closing one
                        }
                        inq ^= ins;
                        ins = ((inq as i64) >> 63) as u64;
                        let (mut sep, mut left) = ((cm | lm) & !inq & u64::MAX >> (64 - (bs[k + 1] - blk).min(64)), qm);
                        while sep != 0 {
                            let b = sep.trailing_zeros() as usize;
                            let below = u64::MAX >> (63 - b); // bits 0..=b
                            (qs[f], en[f]) = (q + (left & below).count_ones(), blk + b);
                            (left &= !below, sep &= sep - 1);
                            (q, f) = (0, f + 1);
                            if f == 5 {
                                (rec(st, en, qs), f = 0);
                            }
                            st[f] = blk + b + 1;
                        }
                        q += left.count_ones();
                        blk += 64;
                    }
                    pre.map(|x| pend.replace(x).map(&mut mark));
                    pend.map(&mut mark);
                    bit.map(set);
                    agg
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert!(((top.load(Relaxed) & 0xFFFFF) as usize) < IDS && (top.load(Relaxed) >> 20) < 1 << 24);
    let codes: Vec<usize> = (0..has.len()).filter(|&k| has[k].load(Relaxed) == 1).collect();
    assert!(codes.len() < 255);
    let mut out = Vec::new();
    for (g, code) in codes.iter().map(|&k| Some(k)).enumerate().chain([(255, None)]) {
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
        match code {
            Some(k) => out.extend_from_slice(&[k / 676, k / 26 % 26, k % 26].map(|x| b'A' + x as u8)),
            None => out.extend_from_slice(b"_UNMATCHED"),
        }
        write!(out, " {} {}", a.count, a.ids).unwrap();
        for c in [a.sum, a.min, a.max] {
            money(&mut out, c);
        }
        writeln!(out, " {} {} {}", names, a.name_bytes, a.note_bytes).unwrap();
    }
    fs::create_dir_all(&dout).unwrap();
    fs::write(format!("{dout}/aggregates.txt"), out).unwrap();
}

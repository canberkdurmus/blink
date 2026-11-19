//! Parallel baseline-JPEG decoder (PERF-4, ADR 0011).
//!
//! A 6 MB embedded preview has ~560k entropy-coded blocks and often no restart markers, so a sequential
//! Huffman decoder alone needs about twice PERF-4's budget. This decoder splits the entropy-coded
//! data into one chunk per thread and uses JPEG's self-synchronisation:
//!
//! 1. **Scan.** Every chunk is parsed from an arbitrary bit offset (no pixels are written).
//!    A decoder that starts mid-stream falls into step with the true decoder after a few blocks,
//!    so each chunk records its first block starts, plus a checkpoint every few MCUs.
//! 2. **Sync.** Each chunk keeps parsing past its end until it reaches a block start recorded by
//!    the next chunk with the same position in the MCU. From there the next chunk's parse is
//!    exact; only its DC predictors differ, by a constant per component, which a serial prefix
//!    pass fixes. Any invalid code inside the true stream rejects the file.
//! 3. **Decode.** The image is cut into bands of MCU rows, one per thread. Each band starts from
//!    the nearest checkpoint, re-parses up to its first MCU, then decodes, inverse-transforms,
//!    upsamples and colour-converts one MCU row at a time into its own slice of the output.
//!    The one luma row at each band edge that needs the neighbour band's chroma is finished
//!    after the threads join.
//!
//! The output matches zune-jpeg within rounding (islow IDCT, triangle chroma upsampling, the
//! same YCbCr → RGB constants). With restart markers (DRI) steps 1–2 are skipped: every
//! restart interval is an exact start point. Anything this decoder doesn't handle (progressive, 12-bit, CMYK/Adobe transforms, other sampling layouts, corrupt data) returns `None`
//! and the caller falls back to zune-jpeg.

#![allow(clippy::needless_range_loop)]

/// Fast Huffman lookup width in bits.
const LB: u32 = 10;
/// Block starts each chunk records for synchronisation.
const SYNC_RECORDS: usize = 1024;
/// Chunks and bands per thread (more only adds overhead: measured on the PERF-4 file).
const PIECES_PER_THREAD: usize = 1;
/// A checkpoint is recorded every this many MCUs.
const CHECKPOINT_MCUS: u32 = 32;
/// Smallest amount of entropy-coded data per chunk.
const MIN_CHUNK_BYTES: usize = 32 * 1024;

const ZZ: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// A Huffman table with a `LB`-bit lookup and a combined AC lookup.
struct Huff {
    /// `(len << 8) | symbol`; 0 means "longer than `LB` bits".
    look: Vec<u16>,
    /// For AC tables: `(value << 16) | ((run + 1) << 8) | (code + extra bit length)`, when code
    /// and extra bits fit in `LB` bits and the symbol isn't EOB/ZRL; 0 otherwise.
    fast_ac: Vec<i32>,
    /// For skipping AC symbols: `(advance in k << 8) | (code + extra bit length)` for every code
    /// of at most `LB` bits (EOB advances 64, ZRL 16); 0 otherwise.
    skip_ac: Vec<u16>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(counts: &[u8; 16], vals: &[u8]) -> Option<Huff> {
        let mut look = vec![0u16; 1 << LB];
        let mut maxcode = [-1i32; 18];
        let mut valptr = [0i32; 17];
        let mut mincode = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0usize;
        for len in 1..=16usize {
            let n = counts[len - 1] as usize;
            valptr[len] = k as i32;
            mincode[len] = code;
            for _ in 0..n {
                if code >= (1 << len) {
                    return None;
                }
                let v = *vals.get(k)?;
                if len as u32 <= LB {
                    let shift = LB - len as u32;
                    let base = (code as usize) << shift;
                    for f in 0..(1usize << shift) {
                        look[base + f] = ((len as u16) << 8) | v as u16;
                    }
                }
                code += 1;
                k += 1;
            }
            maxcode[len] = if n > 0 { code - 1 } else { -1 };
            code <<= 1;
        }
        maxcode[17] = i32::MAX;
        let mut fast_ac = vec![0i32; 1 << LB];
        let mut skip_ac = vec![0u16; 1 << LB];
        for (i, &e) in look.iter().enumerate() {
            if e == 0 {
                continue;
            }
            let len = (e >> 8) as u32;
            let (r, s) = (((e & 0xFF) >> 4) as i32, (e & 15) as u32);
            let adv: u16 = match (r, s) {
                (0, 0) => 64,
                (15, 0) => 16,
                (_, 0) => 0, // invalid: slow path reports it
                _ => r as u16 + 1,
            };
            if adv > 0 {
                skip_ac[i] = (adv << 8) | (len + s) as u16;
            }
            if s == 0 || len + s > LB {
                continue;
            }
            let extra = ((i >> (LB - len - s)) & ((1 << s) - 1)) as i32;
            let v = extend(extra, s);
            fast_ac[i] = (v << 16) | ((r + 1) << 8) | (len + s) as i32;
        }
        Some(Huff {
            look,
            fast_ac,
            skip_ac,
            maxcode,
            valptr,
            mincode,
            vals: vals[..k].to_vec(),
        })
    }
}

#[inline(always)]
fn extend(v: i32, s: u32) -> i32 {
    if s == 0 {
        0
    } else if v < (1 << (s - 1)) {
        v - (1 << s) + 1
    } else {
        v
    }
}

struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    td: usize,
    ta: usize,
}

/// A parsed single-scan baseline JPEG.
struct Frame {
    w: usize,
    h: usize,
    /// Components in scan order.
    comps: Vec<Comp>,
    /// Quantisation tables in zig-zag order.
    q: [[i32; 64]; 4],
    dc: [Option<Huff>; 4],
    ac: [Option<Huff>; 4],
    /// Destuffed entropy-coded data, followed by 8 zero bytes of padding.
    data: Vec<u8>,
    /// Length of `data` without the padding, in bits.
    data_bits: usize,
    /// Restart interval in MCUs (0: none).
    restart: usize,
    /// With a restart interval: bit offset in `data` where each interval starts.
    intervals: Vec<usize>,
}

fn be16(j: &[u8], i: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*j.get(i)?, *j.get(i + 1)?]) as usize)
}

fn parse(j: &[u8]) -> Option<Frame> {
    if j.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    let mut q = [[0i32; 64]; 4];
    let mut dc: [Option<Huff>; 4] = Default::default();
    let mut ac: [Option<Huff>; 4] = Default::default();
    let (mut w, mut h) = (0, 0);
    let mut comps: Vec<Comp> = Vec::new();
    let mut restart = 0;
    loop {
        while *j.get(i)? == 0xFF && *j.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *j.get(i)? != 0xFF {
            return None;
        }
        let m = *j.get(i + 1)?;
        let len = be16(j, i + 2)?;
        if len < 2 {
            return None;
        }
        let seg = j.get(i + 4..i + 2 + len)?;
        match m {
            0xDB => {
                let mut p = 0;
                while p < seg.len() {
                    let (pq, tq) = (seg[p] >> 4, (seg[p] & 15) as usize);
                    if tq > 3 {
                        return None;
                    }
                    p += 1;
                    for k in 0..64 {
                        q[tq][k] = if pq == 0 {
                            p += 1;
                            *seg.get(p - 1)? as i32
                        } else {
                            p += 2;
                            be16(seg, p - 2)? as i32
                        };
                    }
                }
            }
            0xC0 | 0xC1 => {
                if *seg.first()? != 8 || !comps.is_empty() {
                    return None;
                }
                h = be16(seg, 1)?;
                w = be16(seg, 3)?;
                let n = *seg.get(5)? as usize;
                for c in 0..n {
                    let b = seg.get(6 + 3 * c..9 + 3 * c)?;
                    comps.push(Comp {
                        id: b[0],
                        h: (b[1] >> 4) as usize,
                        v: (b[1] & 15) as usize,
                        tq: (b[2] & 15) as usize,
                        td: 0,
                        ta: 0,
                    });
                }
            }
            // Progressive, lossless, hierarchical and arithmetic-coded frames, DNL.
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDC => return None,
            0xC4 => {
                let mut p = 0;
                while p < seg.len() {
                    let (tc, th) = (seg[p] >> 4, (seg[p] & 15) as usize);
                    let counts: [u8; 16] = seg.get(p + 1..p + 17)?.try_into().ok()?;
                    let n: usize = counts.iter().map(|&b| b as usize).sum();
                    let vals = seg.get(p + 17..p + 17 + n)?;
                    if th > 3 || tc > 1 {
                        return None;
                    }
                    let t = Huff::new(&counts, vals)?;
                    if tc == 0 {
                        dc[th] = Some(t);
                    } else {
                        ac[th] = Some(t);
                    }
                    p += 17 + n;
                }
            }
            0xDD => restart = be16(seg, 0)?,
            // Adobe APP14: only the YCbCr transform is handled here.
            0xEE => {
                if seg.starts_with(b"Adobe") && seg.get(11).copied() != Some(1) {
                    return None;
                }
            }
            0xDA => {
                let ns = *seg.first()? as usize;
                if ns != comps.len() || !(ns == 1 || ns == 3) || w == 0 || h == 0 {
                    return None;
                }
                let mut ordered = Vec::with_capacity(ns);
                for s in 0..ns {
                    let (cid, t) = (*seg.get(1 + 2 * s)?, *seg.get(2 + 2 * s)?);
                    let k = comps.iter().position(|c| c.id == cid)?;
                    let mut c = comps.swap_remove(k);
                    c.td = (t >> 4) as usize;
                    c.ta = (t & 15) as usize;
                    if c.td > 3 || c.ta > 3 || c.tq > 3 {
                        return None;
                    }
                    ordered.push(c);
                }
                let (ss, se, a) = (
                    *seg.get(1 + 2 * ns)?,
                    *seg.get(2 + 2 * ns)?,
                    *seg.get(3 + 2 * ns)?,
                );
                if ss != 0 || se != 63 || a != 0 {
                    return None;
                }
                let (data, intervals) = destuff(j, i + 2 + len, restart > 0)?;
                let q = q.map(|t| std::array::from_fn(|k| t[k]));
                return Some(Frame {
                    w,
                    h,
                    comps: ordered,
                    q,
                    dc,
                    ac,
                    data_bits: (data.len() - 8) * 8,
                    data,
                    restart,
                    intervals,
                });
            }
            _ => {}
        }
        i += 2 + len;
    }
}

/// Copy the entropy-coded segment starting at `p`, removing stuffed zero bytes and, when
/// `restarts`, RST markers (returning the bit offset where each restart interval starts).
/// Stops at EOI; returns `None` for any other marker (e.g. a second scan).
fn destuff(j: &[u8], mut p: usize, restarts: bool) -> Option<(Vec<u8>, Vec<usize>)> {
    let mut d = Vec::with_capacity(j.len().saturating_sub(p) + 8);
    let mut intervals = vec![0];
    loop {
        let rest = j.get(p..)?;
        let run = memchr::memchr(0xFF, rest).unwrap_or(rest.len());
        d.extend_from_slice(&rest[..run]);
        p += run;
        if p >= j.len() {
            break; // truncated: no EOI
        }
        match j.get(p + 1) {
            Some(0) => {
                d.push(0xFF);
                p += 2;
            }
            Some(0xFF) => p += 1, // fill byte
            Some(0xD0..=0xD7) if restarts => {
                intervals.push(d.len() * 8);
                p += 2;
            }
            Some(0xD9) | None => break,
            Some(_) => return None,
        }
    }
    d.extend_from_slice(&[0; 8]);
    Some((d, intervals))
}

/// MSB-first bit reader over destuffed data. Reads past the end yield zero bits.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    fn at(d: &'a [u8], bit: usize) -> Self {
        let mut b = Bits {
            d,
            pos: bit / 8,
            buf: 0,
            n: 0,
        };
        b.refill();
        b.skip((bit % 8) as u32);
        b
    }

    /// Top up to at least 56 valid bits.
    #[inline(always)]
    fn refill(&mut self) {
        if let Some(w) = self.d.get(self.pos..self.pos + 8) {
            let w = u64::from_be_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
            self.buf |= w >> self.n;
        }
        self.pos += ((63 - self.n) >> 3) as usize;
        self.n |= 56;
    }

    #[inline(always)]
    fn bitpos(&self) -> usize {
        self.pos * 8 - self.n as usize
    }

    #[inline(always)]
    fn peek(&self, k: u32) -> u32 {
        (self.buf >> (64 - k)) as u32
    }

    #[inline(always)]
    fn skip(&mut self, k: u32) {
        self.buf <<= k;
        self.n -= k;
    }

    /// Decode one Huffman symbol; `None` for an invalid code. Needs ≥ 16 valid bits.
    #[inline(always)]
    fn sym(&mut self, t: &Huff) -> Option<u8> {
        let e = t.look[self.peek(LB) as usize];
        if e != 0 {
            self.skip((e >> 8) as u32);
            return Some(e as u8);
        }
        self.sym_slow(t)
    }

    #[inline(never)]
    fn sym_slow(&mut self, t: &Huff) -> Option<u8> {
        for len in LB as usize + 1..=16 {
            let code = self.peek(len as u32) as i32;
            if code <= t.maxcode[len] {
                self.skip(len as u32);
                return t
                    .vals
                    .get((t.valptr[len] + code - t.mincode[len]) as usize)
                    .copied();
            }
        }
        None
    }

    /// Read `s` (≤ 16) extra bits and sign-extend them. Needs ≥ `s` valid bits.
    #[inline(always)]
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.peek(s) as i32;
        self.skip(s);
        extend(v, s)
    }
}

/// Parser state at a block start.
#[derive(Clone, Copy, Debug, Default)]
struct State {
    bit: usize,
    /// Block index within the MCU.
    b: u8,
    /// Blocks parsed so far by this parser.
    blk: u32,
    /// Invalid codes met so far by this parser.
    bad: u32,
    /// DC predictors (relative to wherever this parser started).
    dc: [i32; 3],
}

/// Huffman tables per block of an MCU.
struct Tables<'a> {
    /// Component of each block in the MCU.
    comp: Vec<usize>,
    dc: Vec<&'a Huff>,
    ac: Vec<&'a Huff>,
}

/// Parse one block, updating `st` (DC predictor, counters). Returns false for an invalid code.
#[inline(always)]
fn skip_block(t: &Tables, bits: &mut Bits, st: &mut State) -> bool {
    let bi = st.b as usize;
    let c = t.comp[bi];
    if bits.n < 32 {
        bits.refill();
    }
    let s = match bits.sym(t.dc[bi]) {
        Some(s) if s <= 15 => s as u32,
        _ => return false,
    };
    st.dc[c] = st.dc[c].wrapping_add(bits.receive_extend(s));
    let ac = t.ac[bi];
    let mut k = 1usize;
    while k < 64 {
        if bits.n < 32 {
            bits.refill();
        }
        let f = ac.skip_ac[bits.peek(LB) as usize];
        if f != 0 {
            bits.skip((f & 0xFF) as u32);
            k += (f >> 8) as usize;
            continue;
        }
        let Some(rs) = bits.sym(ac) else {
            return false;
        };
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += r + 1;
        bits.skip(s);
    }
    st.blk += 1;
    st.b += 1;
    if st.b as usize == t.comp.len() {
        st.b = 0;
    }
    true
}

/// What a chunk's scan produced.
struct Scan {
    /// The first block starts after the chunk's start (for synchronisation).
    sync: Vec<State>,
    /// Checkpoints (MCU starts, every `CHECKPOINT_MCUS` MCUs).
    checkpoints: Vec<State>,
    /// State where the scan stopped.
    end: State,
}

/// Parse from `st` until `stop` returns true at a block start or the data ends.
#[allow(clippy::too_many_arguments)]
fn scan(
    t: &Tables,
    bits: &mut Bits,
    mut st: State,
    data_bits: usize,
    sync: &mut Vec<State>,
    sync_max: usize,
    checkpoints: &mut Vec<State>,
    mut stop: impl FnMut(&State) -> bool,
) -> State {
    let every = CHECKPOINT_MCUS * t.comp.len() as u32;
    let mut last_cp = 0u32;
    loop {
        st.bit = bits.bitpos();
        if sync.len() < sync_max {
            sync.push(st);
        }
        if st.b == 0 && st.blk.wrapping_sub(last_cp) >= every {
            checkpoints.push(st);
            last_cp = st.blk;
        }
        if st.bit >= data_bits || stop(&st) {
            return st;
        }
        // Final padding: only 1-bits left.
        let left = data_bits - st.bit;
        if bits.n < 32 {
            bits.refill();
        }
        if left < 8 && bits.peek(left as u32) == (1u32 << left) - 1 {
            return st;
        }
        if !skip_block(t, bits, &mut st) {
            // Resynchronise one bit later, as a fresh MCU.
            st.bad += 1;
            st.b = 0;
            st.bit = bits.bitpos();
            *bits = Bits::at(bits.d, st.bit + 1);
        }
    }
}

/// Run `f(i, item)` for every item on `workers` threads (the calling thread is one of them).
/// Items are handed out one at a time, so a preempted thread doesn't hold up the others.
/// `None` if any call panicked (only possible on corrupt input; the caller falls back).
fn par_map<T: Send, R: Send>(
    items: Vec<T>,
    workers: usize,
    f: impl Fn(usize, T) -> R + Sync,
) -> Option<Vec<R>> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let n = items.len();
    let slots: Vec<Mutex<Option<T>>> = items.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let results: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let work = || loop {
        let i = next.fetch_add(1, Ordering::Relaxed);
        let Some(slot) = slots.get(i) else { break };
        let item = slot.lock().ok().and_then(|mut s| s.take());
        if let Some(item) = item {
            let r = f(i, item);
            if let Ok(mut out) = results[i].lock() {
                *out = Some(r);
            }
        }
    };
    let ok = std::thread::scope(|sc| {
        let hs: Vec<_> = (1..workers.clamp(1, n.max(1)))
            .map(|_| sc.spawn(work))
            .collect();
        let main = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_ok();
        hs.into_iter().all(|h| h.join().is_ok()) && main
    });
    if !ok {
        return None;
    }
    results
        .into_iter()
        .map(|m| m.into_inner().ok().flatten())
        .collect()
}

/// A verified point in the true stream from which a band can start decoding.
#[derive(Clone, Copy)]
struct Entry {
    /// Global index of the block that starts here.
    global: usize,
    bit: usize,
    /// True DC predictors.
    dc: [i32; 3],
}

/// Decode a baseline JPEG to interleaved RGB8 on up to `threads` threads.
/// Returns `None` when the file isn't handled here (the caller falls back to zune-jpeg).
pub(crate) fn decode(j: &[u8], threads: usize) -> Option<(usize, usize, Vec<u8>)> {
    decode_with(j, threads, MIN_CHUNK_BYTES)
}

/// `decode` with a configurable minimum chunk size (tests use tiny chunks to exercise
/// synchronisation on small images).
fn decode_with(
    j: &[u8],
    threads: usize,
    min_chunk_bytes: usize,
) -> Option<(usize, usize, Vec<u8>)> {
    let f = parse(j)?;
    let ncomp = f.comps.len();
    let (hm, vm) = if ncomp == 1 {
        (1, 1)
    } else {
        let y = &f.comps[0];
        let chroma_1x1 = f.comps[1..].iter().all(|c| (c.h, c.v) == (1, 1));
        if !chroma_1x1 || !(1..=2).contains(&y.h) || !(1..=2).contains(&y.v) {
            return None;
        }
        (y.h, y.v)
    };
    let mcux = f.w.div_ceil(8 * hm);
    let mcuy = f.h.div_ceil(8 * vm);
    // Block layout of an MCU: (component, x block, y block).
    let mut geo = Vec::new();
    for (ci, c) in f.comps.iter().enumerate() {
        let (ch, cv) = if ncomp == 1 { (1, 1) } else { (c.h, c.v) };
        for by in 0..cv {
            for bx in 0..ch {
                geo.push((ci, bx, by));
            }
        }
    }
    let nb = geo.len();
    let t = Tables {
        comp: geo.iter().map(|g| g.0).collect(),
        dc: geo
            .iter()
            .map(|g| f.dc[f.comps[g.0].td].as_ref())
            .collect::<Option<_>>()?,
        ac: geo
            .iter()
            .map(|g| f.ac[f.comps[g.0].ta].as_ref())
            .collect::<Option<_>>()?,
    };
    let workers = threads.clamp(1, 64);
    let entries = if f.restart > 0 {
        restart_entries(&f, mcux * mcuy, nb)?
    } else {
        let n = (workers * PIECES_PER_THREAD).min((f.data.len() / min_chunk_bytes.max(1)).max(1));
        sync_entries(&f, &t, n, workers, mcux * mcuy * nb)?
    };

    // 3. Decode bands of MCU rows.
    let (w, h) = (f.w, f.h);
    let mh = 8 * vm; // luma rows per MCU row
    let mut rgb = vec![0u8; w * h * 3];
    let nbands = (workers * PIECES_PER_THREAD).min(mcuy);
    let band_rows: Vec<usize> = (0..=nbands).map(|k| mcuy * k / nbands).collect();
    let mut outs = Vec::with_capacity(nbands);
    let mut rest = rgb.as_mut_slice();
    for k in 0..nbands {
        let rows = (band_rows[k + 1] * mh).min(h) - (band_rows[k] * mh).min(h);
        let (a, b) = rest.split_at_mut(rows * w * 3);
        outs.push(a);
        rest = b;
    }
    let geom = Geom {
        w,
        h,
        hm,
        vm,
        mcux,
        mcuy,
    };
    let q: Vec<&[i32; 64]> = f.comps.iter().map(|c| &f.q[c.tq]).collect();
    let edges: Vec<Option<Edges>> = par_map(outs, workers, |k, out| {
        let (r0, r1) = (band_rows[k], band_rows[k + 1]);
        decode_band(&t, &f, &entries, &geo, &q, &geom, r0, r1, out)
    })?;
    let edges: Vec<Edges> = edges.into_iter().collect::<Option<_>>()?;

    // Band-edge luma rows whose vertical chroma neighbour lives in the neighbouring band:
    // at every band boundary, the last row of the band above and the first row of the band
    // below (both inside the image, since the band below exists).
    if ncomp == 3 && vm == 2 && nbands > 1 {
        let mut pairs = Vec::with_capacity(nbands - 1);
        let mut rest = rgb.as_mut_slice();
        let mut at = 0;
        for k in 1..nbands {
            let y = band_rows[k] * mh;
            let (_, b) = rest.split_at_mut((y - 1 - at) * w * 3);
            let (pair, b) = b.split_at_mut(2 * w * 3);
            pairs.push((k, pair));
            rest = b;
            at = y + 1;
        }
        let edges = &edges;
        par_map(pairs, workers, |_, (k, pair)| {
            let mut rs = RowScratch::new(w);
            let (above, below) = (&edges[k - 1], &edges[k]);
            let (o_above, o_below) = pair.split_at_mut(w * 3);
            // Odd row: far chroma row = first of the band below.
            colour_row(
                &above.bot_y,
                [&above.bot_c[0], &above.bot_c[1]],
                [&below.top_c[0], &below.top_c[1]],
                hm,
                w,
                &mut rs,
                o_above,
            );
            // Even row: far chroma row = last of the band above.
            colour_row(
                &below.top_y,
                [&below.top_c[0], &below.top_c[1]],
                [&above.bot_c[0], &above.bot_c[1]],
                hm,
                w,
                &mut rs,
                o_below,
            );
        })?;
    }
    Some((w, h, rgb))
}

/// Without restart markers: find verified start points by speculative parallel parsing
/// (steps 1 and 2 of the module docs) over `n` chunks.
fn sync_entries(
    f: &Frame,
    t: &Tables,
    n: usize,
    workers: usize,
    want: usize,
) -> Option<Vec<Entry>> {
    // 1. Scan every chunk from an arbitrary bit offset.
    let starts: Vec<usize> = (0..=n).map(|i| f.data_bits * i / n).collect();
    let scans: Vec<Scan> = par_map((0..n).collect(), workers, |_, i| {
        let mut bits = Bits::at(&f.data, starts[i]);
        let (mut sync, mut checkpoints) = (Vec::new(), Vec::new());
        let sync_max = if i > 0 { SYNC_RECORDS } else { 0 };
        let st = State {
            bit: starts[i],
            ..State::default()
        };
        let end = starts[i + 1];
        let end = scan(
            t,
            &mut bits,
            st,
            f.data_bits,
            &mut sync,
            sync_max,
            &mut checkpoints,
            |s| s.bit >= end,
        );
        Scan {
            sync,
            checkpoints,
            end,
        }
    })?;
    // 2. Extend every chunk until it meets the next chunk's parse.
    // ext[i] = (state where chunk i stops, the matching sync record of chunk i + 1, extra
    // checkpoints found while extending).
    type Ext = Option<(State, Option<State>, Vec<State>)>;
    let ext: Vec<Ext> = par_map((0..n).collect(), workers, |_, i| {
        let st = scans[i].end;
        if i + 1 == n {
            return Some((st, None, Vec::new()));
        }
        let next = &scans[i + 1].sync;
        let mut bits = Bits::at(&f.data, st.bit);
        let mut k = 0;
        let mut hit = None;
        let mut cps = Vec::new();
        let end = scan(
            t,
            &mut bits,
            st,
            f.data_bits,
            &mut Vec::new(),
            0,
            &mut cps,
            |s| {
                while k < next.len() && next[k].bit < s.bit {
                    k += 1;
                }
                if k < next.len() && next[k].bit == s.bit && next[k].b == s.b {
                    hit = Some(next[k]);
                    return true;
                }
                k >= next.len()
            },
        );
        hit.map(|r| (end, Some(r), cps))
    })?;
    // Resolve: the true start of every chunk, its global block range and its DC offsets.
    let mut entries: Vec<Entry> = Vec::new();
    let mut from = State::default();
    let mut g = 0usize;
    let mut dc = [0i32; 3];
    for i in 0..n {
        let (end, hit, cps) = ext[i].as_ref()?;
        if end.bad != from.bad || end.blk < from.blk {
            return None; // an invalid code inside the true stream
        }
        let to_entry = |s: &State| Entry {
            global: g + s.blk.wrapping_sub(from.blk) as usize,
            bit: s.bit,
            dc: std::array::from_fn(|c| dc[c].wrapping_add(s.dc[c].wrapping_sub(from.dc[c]))),
        };
        entries.push(to_entry(&from));
        for s in scans[i].checkpoints.iter().chain(cps) {
            if s.blk > from.blk && s.blk < end.blk {
                entries.push(to_entry(s));
            }
        }
        let e = to_entry(end);
        g = e.global;
        dc = e.dc;
        if let Some(r) = hit {
            from = *r;
        }
    }
    if g < want {
        return None; // truncated
    }
    entries.sort_by_key(|e| e.global);
    Some(entries)
}

/// With restart markers every interval is an exact start point: DC predictors reset to zero.
fn restart_entries(f: &Frame, mcus: usize, nb: usize) -> Option<Vec<Entry>> {
    if f.intervals.len() != mcus.div_ceil(f.restart) {
        return None; // missing or extra RST markers
    }
    let entries: Vec<Entry> = f
        .intervals
        .iter()
        .enumerate()
        .map(|(k, &bit)| Entry {
            global: k * f.restart * nb,
            bit,
            dc: [0; 3],
        })
        .collect();
    entries
        .windows(2)
        .all(|p| p[0].bit <= p[1].bit)
        .then_some(entries)
}

struct Geom {
    w: usize,
    h: usize,
    hm: usize,
    vm: usize,
    mcux: usize,
    mcuy: usize,
}

/// Rows a band hands back for the edge pass (4:2:0 / 4:4:0 only).
#[derive(Default)]
struct Edges {
    top_y: Vec<u8>,
    bot_y: Vec<u8>,
    /// First chroma rows (Cb, Cr) of the band.
    top_c: [Vec<u8>; 2],
    /// Last chroma rows (Cb, Cr) of the band.
    bot_c: [Vec<u8>; 2],
}

/// Decode MCU rows `r0..r1` into `out` (their RGB rows, cropped to the image).
#[allow(clippy::too_many_arguments)]
fn decode_band(
    t: &Tables,
    f: &Frame,
    entries: &[Entry],
    geo: &[(usize, usize, usize)],
    q: &[&[i32; 64]],
    g: &Geom,
    r0: usize,
    r1: usize,
    out: &mut [u8],
) -> Option<Edges> {
    #[cfg(target_arch = "x86_64")]
    if std::arch::is_x86_feature_detected!("avx2") {
        // SAFETY: `decode_band_avx2` only requires AVX2, which the CPU was just checked for
        // (ADR 0011).
        return unsafe { decode_band_avx2(t, f, entries, geo, q, g, r0, r1, out) };
    }
    decode_band_impl(t, f, entries, geo, q, g, r0, r1, out)
}

/// `decode_band_impl` compiled with AVX2: the IDCT and colour conversion vectorise over
/// 8 × i32 lanes (x86-64's SSE2 baseline has no 32-bit vector multiply).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(clippy::too_many_arguments)]
fn decode_band_avx2(
    t: &Tables,
    f: &Frame,
    entries: &[Entry],
    geo: &[(usize, usize, usize)],
    q: &[&[i32; 64]],
    g: &Geom,
    r0: usize,
    r1: usize,
    out: &mut [u8],
) -> Option<Edges> {
    decode_band_impl(t, f, entries, geo, q, g, r0, r1, out)
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn decode_band_impl(
    t: &Tables,
    f: &Frame,
    entries: &[Entry],
    geo: &[(usize, usize, usize)],
    q: &[&[i32; 64]],
    g: &Geom,
    r0: usize,
    r1: usize,
    out: &mut [u8],
) -> Option<Edges> {
    let ncomp = f.comps.len();
    let nb = geo.len();
    if r0 == r1 {
        return Some(Edges::default());
    }
    // Start from the nearest verified point, then parse up to the band's first MCU.
    let first = r0 * g.mcux * nb;
    let e = entries[..entries.partition_point(|e| e.global <= first)].last()?;
    let mut bits = Bits::at(&f.data, e.bit);
    let mut st = State {
        bit: e.bit,
        b: (e.global % nb) as u8,
        dc: e.dc,
        ..State::default()
    };
    for _ in e.global..first {
        if !skip_block(t, &mut bits, &mut st) {
            return None;
        }
    }
    let mut pred = st.dc;

    // MCU-row buffers: component planes for the previous and the current MCU row.
    let mh = 8 * g.vm;
    let pw: Vec<usize> = (0..ncomp)
        .map(|c| {
            if c == 0 {
                g.mcux * g.hm * 8
            } else {
                g.mcux * 8
            }
        })
        .collect();
    let ph: Vec<usize> = (0..ncomp).map(|c| if c == 0 { mh } else { 8 }).collect();
    let mut cur: Vec<Vec<u8>> = (0..ncomp).map(|c| vec![0u8; pw[c] * ph[c]]).collect();
    let mut prev: Vec<Vec<u8>> = cur.clone();
    let (w, h) = (g.w, g.h);
    let mut rs = RowScratch::new(w);
    let mut edges = Edges::default();
    let y_base = r0 * mh;
    let mut coef = [0i32; 64];
    for r in r0..r1 {
        for mx in 0..g.mcux {
            let m = r * g.mcux + mx;
            if f.restart > 0 && m.is_multiple_of(f.restart) {
                bits = Bits::at(&f.data, *f.intervals.get(m / f.restart)?);
                pred = [0; 3];
            }
            for (bi, &(ci, bx, by)) in geo.iter().enumerate() {
                let dct = t.dc[bi];
                let act = t.ac[bi];
                let qt = q[ci];
                if bits.n < 32 {
                    bits.refill();
                }
                let s = bits.sym(dct).unwrap_or(0) as u32;
                pred[ci] = pred[ci].wrapping_add(bits.receive_extend(s.min(16)));
                coef[0] = pred[ci].wrapping_mul(qt[0]);
                let mut nz = false;
                let mut k = 1usize;
                while k < 64 {
                    if bits.n < 32 {
                        bits.refill();
                    }
                    let fa = act.fast_ac[bits.peek(LB) as usize];
                    if fa != 0 {
                        bits.skip((fa & 0xFF) as u32);
                        k += ((fa >> 8) & 0xFF) as usize - 1;
                        if k > 63 {
                            break;
                        }
                        coef[ZZ[k]] = (fa >> 16) * qt[k];
                        nz = true;
                        k += 1;
                        continue;
                    }
                    let Some(rs) = bits.sym(act) else { break };
                    let (rr, s) = ((rs >> 4) as usize, (rs & 15) as u32);
                    if s == 0 {
                        if rr == 15 {
                            k += 16;
                            continue;
                        }
                        break;
                    }
                    k += rr;
                    if k > 63 {
                        break;
                    }
                    coef[ZZ[k]] = bits.receive_extend(s) * qt[k];
                    nz = true;
                    k += 1;
                }
                let stride = pw[ci];
                let (hc, _) = if ci == 0 { (g.hm, g.vm) } else { (1, 1) };
                let x = (mx * hc + bx) * 8;
                let off = by * 8 * stride + x;
                let tile = &mut cur[ci][off..off + 7 * stride + 8];
                if nz {
                    idct(&mut coef, tile, stride);
                } else {
                    let v = ((coef[0].wrapping_shl(2).wrapping_add(16)) >> 5).wrapping_add(128);
                    let v = v.clamp(0, 255) as u8;
                    for row in 0..8 {
                        tile[row * stride..row * stride + 8].fill(v);
                    }
                    coef[0] = 0;
                }
            }
        }
        // Colour this MCU row's luma rows (and the previous one's last row for 4:2:0).
        let yr0 = r * mh;
        let local = |y: usize| (y - y_base) * w * 3;
        if ncomp == 1 {
            for t in 0..mh {
                let y = yr0 + t;
                if y >= h {
                    break;
                }
                let yrow = &cur[0][t * pw[0]..t * pw[0] + w];
                for (px, &v) in out[local(y)..local(y) + w * 3]
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(yrow)
                {
                    *px = [v, v, v];
                }
            }
        } else {
            let cw = pw[1];
            fn row(p: &[u8], cw: usize, cy: usize) -> &[u8] {
                &p[cy * cw..cy * cw + cw]
            }
            if g.vm == 1 {
                for t in 0..mh {
                    let y = yr0 + t;
                    if y >= h {
                        break;
                    }
                    let near = [row(&cur[1], cw, t), row(&cur[2], cw, t)];
                    colour_row(
                        &cur[0][t * pw[0]..t * pw[0] + w],
                        near,
                        near,
                        g.hm,
                        w,
                        &mut rs,
                        &mut out[local(y)..local(y) + w * 3],
                    );
                }
            } else {
                // Last luma row of the previous MCU row: far = this row's first chroma row.
                if r > r0 {
                    let y = yr0 - 1;
                    if y < h {
                        let near = [row(&prev[1], cw, 7), row(&prev[2], cw, 7)];
                        let far = [row(&cur[1], cw, 0), row(&cur[2], cw, 0)];
                        colour_row(
                            &prev[0][15 * pw[0]..15 * pw[0] + w],
                            near,
                            far,
                            g.hm,
                            w,
                            &mut rs,
                            &mut out[local(y)..local(y) + w * 3],
                        );
                    }
                }
                for t in 0..16 {
                    let y = yr0 + t;
                    if y >= h {
                        break;
                    }
                    let cy = t / 2;
                    let near = [row(&cur[1], cw, cy), row(&cur[2], cw, cy)];
                    let far = if t % 2 == 0 {
                        if cy > 0 {
                            [row(&cur[1], cw, cy - 1), row(&cur[2], cw, cy - 1)]
                        } else if r > r0 {
                            [row(&prev[1], cw, 7), row(&prev[2], cw, 7)]
                        } else if r == 0 {
                            near // image top: clamp
                        } else {
                            continue; // band top: done in the edge pass
                        }
                    } else if cy < 7 {
                        // May be MCU padding below the image, as in zune-jpeg.
                        [row(&cur[1], cw, cy + 1), row(&cur[2], cw, cy + 1)]
                    } else if r + 1 == g.mcuy {
                        near // bottom of the last MCU row: clamp
                    } else {
                        continue; // the next MCU row (or band) provides it
                    };
                    colour_row(
                        &cur[0][t * pw[0]..t * pw[0] + w],
                        near,
                        far,
                        g.hm,
                        w,
                        &mut rs,
                        &mut out[local(y)..local(y) + w * 3],
                    );
                }
                if r == r0 {
                    edges.top_y = cur[0][..w].to_vec();
                    edges.top_c = [row(&cur[1], cw, 0).to_vec(), row(&cur[2], cw, 0).to_vec()];
                }
                if r + 1 == r1 {
                    edges.bot_y = cur[0][15 * pw[0]..15 * pw[0] + w].to_vec();
                    edges.bot_c = [row(&cur[1], cw, 7).to_vec(), row(&cur[2], cw, 7).to_vec()];
                }
            }
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Some(edges)
}

/// Upsample one chroma row pair (triangle filter) and convert one luma row to RGB.
/// `near`/`far` are the nearest and second-nearest chroma rows (equal when not subsampled
/// vertically); `hm` is the horizontal subsampling factor (1 or 2). `s` is scratch.
#[inline(always)]
fn colour_row(
    yrow: &[u8],
    near: [&[u8]; 2],
    far: [&[u8]; 2],
    hm: usize,
    w: usize,
    s: &mut RowScratch,
    out: &mut [u8],
) {
    let half = w.div_ceil(2);
    let RowScratch { cb, cr, col } = s;
    for (dst, (n, f)) in [&mut *cb, &mut *cr]
        .into_iter()
        .zip(near.iter().zip(far.iter()))
    {
        if hm == 1 {
            for ((d, &a), &b) in dst[..w].iter_mut().zip(&n[..w]).zip(&f[..w]) {
                *d = (3 * a as i32 + b as i32 + 2) >> 2;
            }
            continue;
        }
        // col = [c0, c0, c1, …, c(half-1), c(half)]: the left edge replicates; on the right
        // the MCU padding column is used when there is one (as zune-jpeg does).
        let col = &mut col[..half + 2];
        for ((c, &a), &b) in col[1..=half].iter_mut().zip(&n[..half]).zip(&f[..half]) {
            *c = 3 * a as i32 + b as i32;
        }
        col[0] = col[1];
        col[half + 1] = match (n.get(half), f.get(half)) {
            (Some(&a), Some(&b)) => 3 * a as i32 + b as i32,
            _ => col[half],
        };
        let (l, m, r) = (&col[..half], &col[1..=half], &col[2..half + 2]);
        let pairs = w / 2;
        for (((d, &l), &m), &r) in dst[..2 * pairs]
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(l)
            .zip(m)
            .zip(r)
        {
            d[0] = (3 * m + l + 8) >> 4;
            d[1] = (3 * m + r + 7) >> 4;
        }
        if w % 2 == 1 {
            dst[w - 1] = (3 * m[half - 1] + l[half - 1] + 8) >> 4;
        }
    }
    const N: usize = 16;
    let body = w / N * N;
    let (ob, ot) = out[..w * 3].split_at_mut(body * 3);
    for (((o, y), b), r) in ob
        .as_chunks_mut::<{ 3 * N }>()
        .0
        .iter_mut()
        .zip(yrow[..body].as_chunks::<N>().0)
        .zip(cb[..body].as_chunks::<N>().0)
        .zip(cr[..body].as_chunks::<N>().0)
    {
        let (mut rr, mut gg, mut bb) = ([0i32; N], [0i32; N], [0i32; N]);
        for k in 0..N {
            let yy = ((y[k] as i32) << 16) + 32768;
            let (cb, cr) = (b[k] - 128, r[k] - 128);
            rr[k] = (yy + 91881 * cr) >> 16;
            gg[k] = (yy - 22554 * cb - 46802 * cr) >> 16;
            bb[k] = (yy + 116130 * cb) >> 16;
        }
        let mut ob = [0u8; 3 * N];
        for k in 0..N {
            ob[3 * k] = rr[k].clamp(0, 255) as u8;
            ob[3 * k + 1] = gg[k].clamp(0, 255) as u8;
            ob[3 * k + 2] = bb[k].clamp(0, 255) as u8;
        }
        *o = ob;
    }
    for (((o, &y), &b), &r) in ot
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(&yrow[body..w])
        .zip(&cb[body..w])
        .zip(&cr[body..w])
    {
        *o = ycc(y, b, r);
    }
}

/// YCbCr → RGB (JFIF, 16-bit fixed point, as zune-jpeg).
#[inline(always)]
fn ycc(y: u8, cb: i32, cr: i32) -> [u8; 3] {
    let yy = ((y as i32) << 16) + 32768;
    let (b, r) = (cb - 128, cr - 128);
    [
        ((yy + 91881 * r) >> 16).clamp(0, 255) as u8,
        ((yy - 22554 * b - 46802 * r) >> 16).clamp(0, 255) as u8,
        ((yy + 116130 * b) >> 16).clamp(0, 255) as u8,
    ]
}

/// Per-thread scratch for `colour_row`.
struct RowScratch {
    cb: Vec<i32>,
    cr: Vec<i32>,
    col: Vec<i32>,
}

impl RowScratch {
    fn new(w: usize) -> Self {
        RowScratch {
            cb: vec![0; w],
            cr: vec![0; w],
            col: vec![0; w.div_ceil(2) + 2],
        }
    }
}

/// libjpeg "islow" integer IDCT of one dequantised block (natural order) into an 8×8 tile.
/// Clears `inp` for the next block. Written over whole rows so it vectorises.
#[inline(always)]
fn idct(inp: &mut [i32; 64], out: &mut [u8], stride: usize) {
    let mut rows = [[0i32; 8]; 8];
    for r in 0..8 {
        rows[r].copy_from_slice(&inp[r * 8..r * 8 + 8]);
    }
    *inp = [0; 64];
    // Pass 1: columns (lanes = columns).
    let ws = idct8(&rows, 11, 0);
    // Pass 2: rows (transpose so lanes = rows).
    let mut tr = [[0i32; 8]; 8];
    for r in 0..8 {
        for c in 0..8 {
            tr[c][r] = ws[r][c];
        }
    }
    let o = idct8(&tr, 18, 128);
    for r in 0..8 {
        let dst = &mut out[r * stride..r * stride + 8];
        for c in 0..8 {
            dst[c] = o[c][r].clamp(0, 255) as u8;
        }
    }
}

/// One 1-D islow IDCT across 8 lanes: `x[k]` holds frequency k of each lane.
/// Output `(value + round) >> shift`, plus `bias`.
#[inline(always)]
fn idct8(x: &[[i32; 8]; 8], shift: u32, bias: i32) -> [[i32; 8]; 8] {
    const F0298: i32 = 2446;
    const F0390: i32 = 3196;
    const F0541: i32 = 4433;
    const F0765: i32 = 6270;
    const F0899: i32 = 7373;
    const F1175: i32 = 9633;
    const F1501: i32 = 12299;
    const F1847: i32 = 15137;
    const F1961: i32 = 16069;
    const F2053: i32 = 16819;
    const F2562: i32 = 20995;
    const F3072: i32 = 25172;
    let rd = 1 << (shift - 1);
    let mut o = [[0i32; 8]; 8];
    for l in 0..8 {
        let z1 = (x[2][l] + x[6][l]) * F0541;
        let tmp2 = z1 - x[6][l] * F1847;
        let tmp3 = z1 + x[2][l] * F0765;
        let tmp0 = (x[0][l] + x[4][l]) << 13;
        let tmp1 = (x[0][l] - x[4][l]) << 13;
        let t10 = tmp0 + tmp3 + rd;
        let t13 = tmp0 - tmp3 + rd;
        let t11 = tmp1 + tmp2 + rd;
        let t12 = tmp1 - tmp2 + rd;
        let (a0, a1, a2, a3) = (x[7][l], x[5][l], x[3][l], x[1][l]);
        let z1 = a0 + a3;
        let z2 = a1 + a2;
        let z3 = a0 + a2;
        let z4 = a1 + a3;
        let z5 = (z3 + z4) * F1175;
        let z1 = z1 * -F0899;
        let z2 = z2 * -F2562;
        let z3 = z3 * -F1961 + z5;
        let z4 = z4 * -F0390 + z5;
        let o0 = a0 * F0298 + z1 + z3;
        let o1 = a1 * F2053 + z2 + z4;
        let o2 = a2 * F3072 + z2 + z3;
        let o3 = a3 * F1501 + z1 + z4;
        o[0][l] = ((t10 + o3) >> shift) + bias;
        o[7][l] = ((t10 - o3) >> shift) + bias;
        o[1][l] = ((t11 + o2) >> shift) + bias;
        o[6][l] = ((t11 - o2) >> shift) + bias;
        o[2][l] = ((t12 + o1) >> shift) + bias;
        o[5][l] = ((t12 - o1) >> shift) + bias;
        o[3][l] = ((t13 + o0) >> shift) + bias;
        o[4][l] = ((t13 - o0) >> shift) + bias;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::decode_with;
    use crate::image::decode_jpeg_zune;
    use blink_testkit::fixtures::{JpegOpts, jpeg_with, photo_rgb};

    /// Every layout the parallel decoder handles.
    fn layouts() -> Vec<(&'static str, JpegOpts)> {
        let o = JpegOpts::default();
        vec![
            ("4:2:0", o),
            (
                "4:2:2",
                JpegOpts {
                    sampling: (2, 1),
                    ..o
                },
            ),
            (
                "4:4:0",
                JpegOpts {
                    sampling: (1, 2),
                    ..o
                },
            ),
            (
                "4:4:4",
                JpegOpts {
                    sampling: (1, 1),
                    ..o
                },
            ),
            ("gray", JpegOpts { gray: true, ..o }),
            (
                "4:2:0 RST every 4 MCUs",
                JpegOpts {
                    restart_interval: 4,
                    ..o
                },
            ),
            (
                "4:2:2 RST every 7 MCUs",
                JpegOpts {
                    sampling: (2, 1),
                    restart_interval: 7,
                    ..o
                },
            ),
            (
                "gray RST every MCU",
                JpegOpts {
                    gray: true,
                    restart_interval: 1,
                    ..o
                },
            ),
        ]
    }

    /// Sizes with partial MCUs on both axes, odd and even.
    const SIZES: [(u32, u32); 4] = [(333, 251), (256, 160), (98, 403), (98, 250)];

    fn max_mean_diff(a: &[u8], b: &[u8]) -> (u32, f64) {
        assert_eq!(a.len(), b.len());
        let mut max = 0;
        let mut sum = 0u64;
        for (x, y) in a.iter().zip(b) {
            let d = (*x as i32 - *y as i32).unsigned_abs();
            max = max.max(d);
            sum += d as u64;
        }
        (max, sum as f64 / a.len() as f64)
    }

    #[test]
    fn req_perf_4_output_is_identical_for_any_thread_and_chunk_count() {
        for (name, o) in layouts() {
            for (w, h) in SIZES {
                let j = jpeg_with(&photo_rgb(w, h), w, h, o);
                let one = decode_with(&j, 1, usize::MAX)
                    .unwrap_or_else(|| panic!("{name} {w}x{h}: rejected"));
                assert_eq!((one.0, one.1), (w as usize, h as usize));
                for threads in [2, 3, 5, 8, 13] {
                    // ~256-byte chunks: many chunks, each synchronising mid-stream.
                    let many = decode_with(&j, threads, 256)
                        .unwrap_or_else(|| panic!("{name} {w}x{h} ×{threads}: rejected"));
                    assert!(
                        one.2 == many.2,
                        "{name} {w}x{h}: {threads} threads differ from 1 (max/mean diff {:?})",
                        max_mean_diff(&one.2, &many.2)
                    );
                }
            }
        }
    }

    #[test]
    fn req_perf_4_matches_zune_jpeg_within_rounding() {
        for (name, o) in layouts() {
            for (w, h) in SIZES {
                let j = jpeg_with(&photo_rgb(w, h), w, h, o);
                let ours = decode_with(&j, 4, 256).expect("handled");
                let zune = decode_jpeg_zune(&j).expect("zune decodes").image;
                let (max, mean) = max_mean_diff(&ours.2, &zune.data);
                assert!(
                    max <= 8 && mean < 0.5,
                    "{name} {w}x{h}: max {max}, mean {mean:.3} vs zune-jpeg"
                );
            }
        }
    }

    #[test]
    fn req_perf_4_unhandled_layouts_are_left_to_zune_jpeg() {
        let (w, h) = (200, 120);
        let px = photo_rgb(w, h);
        let o = JpegOpts::default();
        for (name, o) in [
            (
                "progressive",
                JpegOpts {
                    progressive: true,
                    ..o
                },
            ),
            (
                "4:1:1",
                JpegOpts {
                    sampling: (4, 1),
                    ..o
                },
            ),
        ] {
            let j = jpeg_with(&px, w, h, o);
            assert!(
                decode_with(&j, 4, 256).is_none(),
                "{name}: should fall back"
            );
        }
        assert!(decode_with(b"not a jpeg", 4, 256).is_none());
    }

    #[test]
    fn req_perf_4_corrupt_or_truncated_data_never_panics() {
        let (w, h) = (160, 120);
        let plain = jpeg_with(&photo_rgb(w, h), w, h, JpegOpts::default());
        let rst = JpegOpts {
            restart_interval: 3,
            ..JpegOpts::default()
        };
        let rst = jpeg_with(&photo_rgb(w, h), w, h, rst);
        let check = |b: &[u8]| {
            if let Some((dw, dh, px)) = decode_with(b, 4, 256) {
                assert_eq!((dw, dh, px.len()), (w as usize, h as usize, dw * dh * 3));
            }
        };
        for j in [&plain, &rst] {
            for cut in (0..j.len()).step_by(97) {
                check(&j[..cut]);
            }
        }
        let mut seed = 7u32;
        for round in 0..600 {
            let mut b = if round % 2 == 0 {
                plain.clone()
            } else {
                rst.clone()
            };
            for _ in 0..3 {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let i = (seed as usize >> 4) % b.len();
                b[i] ^= 1 << (seed & 7);
            }
            check(&b);
        }
    }
}

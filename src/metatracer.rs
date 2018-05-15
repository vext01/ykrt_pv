// Copyright (c) 2017 King's College London
// created by the Software Development Team <http://soft-dev.org/>
//
// The Universal Permissive License (UPL), Version 1.0
//
// Subject to the condition set forth below, permission is hereby granted to any person obtaining a
// copy of this software, associated documentation and/or data (collectively the "Software"), free
// of charge and under any and all copyright rights in the Software, and any and all patent rights
// owned or freely licensable by each licensor hereunder covering either (i) the unmodified
// Software as contributed to or provided by such licensor, or (ii) the Larger Works (as defined
// below), to deal in both
//
// (a) the Software, and
// (b) any piece of software and/or hardware listed in the lrgrwrks.txt file
// if one is included with the Software (each a "Larger Work" to which the Software is contributed
// by such licensors),
//
// without restriction, including without limitation the rights to copy, create derivative works
// of, display, perform, and distribute the Software and make, use, sell, offer for sale, import,
// export, have made, and have sold the Software and the Larger Work(s), and to sublicense the
// foregoing rights on either these or other terms.
//
// This license is subject to the following condition: The above copyright notice and either this
// complete permission notice or at a minimum a reference to the UPL must be included in all copies
// or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
// BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
// NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
// DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

use std::{
    cell::RefCell,
    sync::{atomic::{AtomicU32, Ordering}, Arc, RwLock},
    thread,
};
use hwtracer::{
    Trace, Tracer, HWTracerError, TracerState,
    backends::{perf_pt::PerfPTTracer, dummy::DummyTracer},
};

thread_local! {
    // Each thread has its own tracer, initialised on first use.
    pub static THREAD_TRACER: RefCell<Option<Box<Tracer>>> = RefCell::new(None);
}

// Execute a closure which accepts the current thread's tracer as its sole argument.
//
// If the tracer doesn't exist, it is created.
fn with_thread_tracer<F>(f: F) where F: FnOnce(&mut Box<Tracer>) {
    THREAD_TRACER.with(|key| {
        let mut opt_box_tr = key.borrow_mut();
        if opt_box_tr.is_none() {
            // The tracer for this thread has not been initialised yet. Make it.
            *opt_box_tr = Some(tracer());
        }
        f(opt_box_tr.as_mut().unwrap())
    });
}

/// Instantiate a tracer suitable for the current platform.
fn tracer() -> Box<Tracer> {
    if cfg!(target_os = "linux") {
        match PerfPTTracer::new(PerfPTTracer::config()) {
            Ok(ptt) => return Box::new(ptt),
            Err(e) => {
                eprintln!("Warning: software or hardware doesn't support Intel PT. \
                          Using dummy backend: {}", e);
                return Box::new(DummyTracer::new())
            }
        }
    }
    eprintln!("Warning: No backend for this OS. Using dummy backend");
    return Box::new(DummyTracer::new());
}

pub type HotThreshold = u32;
const DEFAULT_HOT_THRESHOLD: HotThreshold = 50;

// The current meta-tracing phase of a given location in the end-user's code. Consists of a tag and
// (optionally) a value. The tags are in the high order bits since we expect the most common tag is
// PHASE_COMPILED which has an index associated with it. By also making that tag 0b00, we allow
// that index to be accessed without any further operations after the initial tag check.
const PHASE_TAG     : u32 = 0b11 << 30; // All of the other PHASE_ tags must fit in this.
const PHASE_COMPILED: u32 = 0b00 << 30; // The value specifies the compiled trace ID.
const PHASE_TRACING : u32 = 0b01 << 30;
const PHASE_COUNTING: u32 = 0b10 << 30; // The value specifies the current hot count.
const PHASE_COMPILING: u32 = 0b11 << 30;

/// A compiled native code trace.
struct Code {}

impl Code {
    fn compile(_: &Box<Trace>) -> Self {
        // XXX actually compile the trace.
        Self {}
    }

    fn execute(&self) {
        // XXX actually execute the trace.
    }
}

/// A `Location` uniquely identifies a control point position in the end-user's program (and is
/// used by the `MetaTracer` to store data about that location). In other words, every position
/// that can be a control point also needs to have one `Location` value associated with it, and
/// that same `Location` value must always be used to identify that control point.
///
/// As this may suggest, program positions that can't be control points don't need an associated
/// `Location`. For interpreters that can't (or don't want) to be as selective, a simple (if
/// moderately wasteful) mechanism is for every bytecode or AST node to have its own `Location`
/// (even for bytecodes or nodes that can't be control points).
///
/// Location is shareable amongst threads.
pub struct Location {
    pack: Arc<AtomicU32>,
}

impl Location {
    /// Create a fresh Location suitable for passing to `MetaTracer::control_point`.
    pub fn new() -> Self {
        Self { pack: Arc::new(AtomicU32::new(PHASE_COUNTING)) }
    }
}

/// A meta-tracer.
pub struct MetaTracer {
    hot_threshold: HotThreshold,
    code_cache: Arc<RwLock<Vec<Code>>>, // Compiled traces. Indices serve as unique identifiers.
}

impl MetaTracer {
    /// Create a new `MetaTracer` with default settings.
    pub fn new() -> Self {
        Self::new_with_hot_threshold(DEFAULT_HOT_THRESHOLD)
    }

    /// Create a new `MetaTracer` with a specific hot threshold.
    pub fn new_with_hot_threshold(hot_threshold: HotThreshold) -> Self {
        Self {
            hot_threshold: hot_threshold,
            code_cache: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Attempt to execute a compiled trace for location `loc`.
    pub fn control_point(&self, loc: &Location) {
        // Since we don't hold an explicit lock, updating a Location is tricky: we might read a
        // Location, work out what we'd like to update it to, and try updating it, only to find
        // that another thread interrupted us part way through. We therefore use compare_and_swap
        // to update values, allowing us to detect if we were interrupted. If we were interrupted,
        // we simply retry the whole operation.
        let mut trace: Option<Box<Trace>> = None;
        loop {
            let pack = &loc.pack;
            let lp = pack.load(Ordering::Acquire);
            match lp & PHASE_TAG {
                PHASE_COUNTING => {
                    let count = lp & !PHASE_TAG;
                    if count >= self.hot_threshold {
                        if pack.compare_and_swap(lp, PHASE_TRACING, Ordering::Release) == lp {
                            // Only start tracing once updating the pack has succeeded to avoid
                            // tracing multiple iterations of the phase update loop.
                            let mut res = Ok(());
                            with_thread_tracer(|tracer| res = tracer.start_tracing());
                            match res {
                                Ok(_) => break,
                                Err(HWTracerError::TracerState(TracerState::Started)) => {
                                    // This thread's tracer is already tracing something (it even
                                    // could have been initiated from another instance of
                                    // MetaTracer in the same thread). Roll back the pack to as it
                                    // was before. We can't clash with another thread at this
                                    // point, as no one else could have mutated this location since
                                    // we tagged it PHASE_TRACING.
                                    pack.store(PHASE_COUNTING | count, Ordering::Relaxed);
                                    break;
                                },
                                Err(e) => unreachable!("{}", e),
                            }
                        }
                    } else {
                        let new_pack = PHASE_COUNTING | (count + 1);
                        if pack.compare_and_swap(lp, new_pack, Ordering::Release) == lp {
                            break;
                        }
                    }
                },
                PHASE_TRACING => {
                    if trace.is_none() {
                        // It wouldn't make sense to stop tracing multiple times, so stash the
                        // trace first time around the pack update loop only.
                        let mut res = Err(HWTracerError::Unknown);
                        with_thread_tracer(|tracer| res = tracer.stop_tracing());
                        match res {
                            Ok(t) => {
                                // We've arrived back at the Location from which we started tracing
                                // and the current thread is the one that started the tracing.
                                trace = Some(t);
                            },
                            Err(HWTracerError::TracerState(TracerState::Stopped)) => {
                                // Another thread is tracing this location. Do nothing.
                                break;
                            },
                            Err(e) => panic!("Unexpected tracer error: {}", e),
                        }
                    }
                    if pack.compare_and_swap(lp, PHASE_COMPILING, Ordering::Release) == lp {
                        // Compilation is done in a background thread.
                        let thr_pack = Arc::clone(&pack);
                        let thr_code_cache = Arc::clone(&self.code_cache);
                        thread::spawn(move || {
                            let code = Code::compile(&trace.unwrap());
                            let mut thr_code_cache_w = thr_code_cache.write().unwrap();
                            // Store the index of where the new code exists in the code cache into
                            // the value part of the pack so we can look it up in constant time.
                            let idx = thr_code_cache_w.len() as u32;
                            thr_code_cache_w.push(code);
                            // We don't need a retry loop here. Once a location enters
                            // PHASE_COMPILING, no-one else can be mutating at the same time.
                            // Release ordering prevents a potential out-of-bounds Vector access.
                            // Another thread must not see `PHASE_COMPILED | idx` before an entry
                            // with index `idx` has actually entered the code cache.
                            thr_pack.store(PHASE_COMPILED | idx, Ordering::Release);
                        });
                        break;
                    }
                },
                PHASE_COMPILING => {
                    // We are still waiting for compiled code to appear.
                    break;
                },
                PHASE_COMPILED => {
                    self.code_cache.read().unwrap()[lp as usize].execute();
                    break;
                }
                _ => unreachable!()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use test::{Bencher, black_box};
    use super::*;

    #[test]
    fn threshold_passed() {
        let hot_thrsh = 1500;
        let mt = MetaTracer::new_with_hot_threshold(hot_thrsh);
        let lp = Location::new();
        for i in 0..hot_thrsh {
            mt.control_point(&lp);
            assert_eq!(lp.pack.load(Ordering::Relaxed), PHASE_COUNTING | (i + 1));
        }
        mt.control_point(&lp);
        assert_eq!(lp.pack.load(Ordering::Relaxed), PHASE_TRACING);
        mt.control_point(&lp);
        let pack = lp.pack.load(Ordering::Relaxed);
        assert!(pack == PHASE_COMPILING || pack & PHASE_TAG == PHASE_COMPILED);
    }

    // Test using two MetaTracer instances in the same thread.
    #[test]
    fn threshold_passed_multiple_metatracers() {
        let hot_thrsh = 4000;
        let mt1 = MetaTracer::new_with_hot_threshold(hot_thrsh);
        let mt2 = MetaTracer::new_with_hot_threshold(hot_thrsh);
        let loc1 = Location::new();
        let loc2 = Location::new();

        let mut lp1;
        let mut lp2;
        // Get both locations to one less than their hot threshold.
        for _ in 0..hot_thrsh {
            mt1.control_point(&loc1);
            mt2.control_point(&loc2);
            lp1 = loc1.pack.load(Ordering::Relaxed);
            lp2 = loc2.pack.load(Ordering::Relaxed);
            assert_eq!(lp1 & PHASE_TAG, PHASE_COUNTING);
            assert_eq!(lp2 & PHASE_TAG, PHASE_COUNTING);
        }

        // mt1 starts tracing, mt2 wants to, but our thread's tracer is busy.
        mt1.control_point(&loc1);
        mt2.control_point(&loc2);
        lp1 = loc1.pack.load(Ordering::Relaxed);
        lp2 = loc2.pack.load(Ordering::Relaxed);
        assert_eq!(lp1 & PHASE_TAG, PHASE_TRACING);
        assert_eq!(lp2 & PHASE_TAG, PHASE_COUNTING);

        // mt1 finishes tracing, freeing up our thread's tracer for mt2.
        mt1.control_point(&loc1);
        mt2.control_point(&loc2);
        lp1 = loc1.pack.load(Ordering::Relaxed);
        lp2 = loc2.pack.load(Ordering::Relaxed);
        assert!(lp1 == PHASE_COMPILING || lp1 & PHASE_TAG == PHASE_COMPILED);
        assert_eq!(lp2 & PHASE_TAG, PHASE_TRACING);

        // Once more and mt2 is finished tracing too.
        mt2.control_point(&loc2);
        lp2 = loc2.pack.load(Ordering::Relaxed);
        assert!(lp2 == PHASE_COMPILING || lp2 & PHASE_TAG == PHASE_COMPILED);
    }

    #[test]
    fn threaded_threshold_passed() {
        let hot_thrsh = 4000;
        let mt_arc;
        {
            let mt = MetaTracer::new_with_hot_threshold(hot_thrsh);
            mt_arc = Arc::new(mt);
        }
        let lp_arc = Arc::new(Location::new());
        let mut thrs = vec![];
        for _ in 0..hot_thrsh / 4 {
            let mt_arc_cl = Arc::clone(&mt_arc);
            let lp_arc_cl = Arc::clone(&lp_arc);
            let t = thread::Builder::new()
                .spawn(move || {
                    mt_arc_cl.control_point(&*lp_arc_cl);
                    let c1 = lp_arc_cl.pack.load(Ordering::Relaxed);
                    assert_eq!(c1 & PHASE_TAG, PHASE_COUNTING);
                    mt_arc_cl.control_point(&*lp_arc_cl);
                    let c2 = lp_arc_cl.pack.load(Ordering::Relaxed);
                    assert_eq!(c2 & PHASE_TAG, PHASE_COUNTING);
                    mt_arc_cl.control_point(&*lp_arc_cl);
                    let c3 = lp_arc_cl.pack.load(Ordering::Relaxed);
                    assert_eq!(c3 & PHASE_TAG, PHASE_COUNTING);
                    mt_arc_cl.control_point(&*lp_arc_cl);
                    let c4 = lp_arc_cl.pack.load(Ordering::Relaxed);
                    assert_eq!(c4 & PHASE_TAG, PHASE_COUNTING);
                    assert!(c4 > c3);
                    assert!(c3 > c2);
                    assert!(c2 > c1);
                })
                .unwrap();
            thrs.push(t);
        }
        for t in thrs {
            t.join().unwrap();
        }
        mt_arc.control_point(&lp_arc);
        assert_eq!(lp_arc.pack.load(Ordering::Relaxed), PHASE_TRACING);
        mt_arc.control_point(&lp_arc);
        let lp = lp_arc.pack.load(Ordering::Relaxed);
        assert!(lp == PHASE_COMPILING || lp & PHASE_TAG == PHASE_COMPILED);
    }

    #[bench]
    fn bench_single_threaded_control_point(b: &mut Bencher) {
        let mt = MetaTracer::new();
        let lp = Location::new();
        b.iter(|| {
            for _ in 0..100000 {
                black_box(mt.control_point(&lp));
            }
        });
    }

    #[bench]
    fn bench_multi_threaded_control_point(b: &mut Bencher) {
        let mt_arc = Arc::new(MetaTracer::new());
        let lp_arc = Arc::new(Location::new());
        b.iter(|| {
            let mut thrs = vec![];
            for _ in 0..4 {
                let mt_arc_cl = Arc::clone(&mt_arc);
                let lp_arc_cl = Arc::clone(&lp_arc);
                let t = thread::Builder::new()
                    .spawn(move || {
                        for _ in 0..100000 {
                            black_box(mt_arc_cl.control_point(&*lp_arc_cl));
                        }
                    })
                    .unwrap();
                thrs.push(t);
            }
            for t in thrs {
                t.join().unwrap();
            }
        });
    }
}

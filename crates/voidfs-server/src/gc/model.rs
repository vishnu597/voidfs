// SPDX-License-Identifier: Apache-2.0
//! A model of the garbage-collection protocol (format §12, RFC 0002), checked by exploring every
//! interleaving of a collector, writers and drive deletions in small configurations.
//!
//! The model abstracts the bucket to a few objects with modification times, the pool's roots
//! (drives, checkpoints, staging records) to sets of references, and time to integer ticks. It
//! checks the protocol's safety property: no live root ever references a missing object. Each
//! variant that drops one rule must fail, which shows that the exploration finds the races the
//! rules exist to close.
//!
//! Each writer follows a script of operations; the exploration covers every timing of the
//! scripts against the collector, the clock and the deletion of roots. The races come from
//! timing, and fixing the operations keeps the state space small enough to explore completely.
//!
//! Integer time makes events in the same tick look simultaneous, so the model's writer commits
//! strictly before `r + grace / 2`; the specification's "no more than" is for real time.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

const MAX_HASHES: usize = 4;
const MAX_ROOTS: usize = 3;
const MAX_WRITERS: usize = 2;

/// Which rules the collector and the writers follow.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Phase 2 marks the run `deleting` before it recomputes (§12.3 step 1).
    pub mark_deleting: bool,
    /// Phase 1 is abandoned if it takes longer than `grace / 2` (§12.2 step 5).
    pub bound_phase1: bool,
    /// A writer that rescued a candidate re-reads the run record before committing (§12.4, 3).
    pub recheck_rescue: bool,
    /// Uploads need the §12.4 checks too (§7.4); draft 1 counted an upload as confirmation.
    pub check_uploads: bool,
    /// Writers reuse any object they have ever written or read, with no checks: the server's
    /// behaviour before garbage collection existed.
    pub unchecked_cache: bool,
}

impl Rules {
    /// Format draft 1, revision 2 (RFC 0002).
    pub const AMENDED: Rules = Rules { mark_deleting: true, bound_phase1: true, recheck_rescue: true, check_uploads: true, unchecked_cache: false };
    /// Format draft 1 as first written.
    pub const DRAFT1: Rules = Rules { mark_deleting: false, bound_phase1: false, recheck_rescue: false, check_uploads: false, unchecked_cache: false };
}

/// One step of a writer's script.
#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Commit a reference to object `x` in root `k`, uploading it unless a check allows reuse.
    Put { x: u8, k: u8 },
    /// Read root `j` and, if it references `x`, commit a reference to `x` in root `k` (§12.4, 1).
    Copy { j: u8, x: u8, k: u8 },
    /// GET object `x` (§12.4, 2).
    Read { x: u8 },
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub rules: Rules,
    /// Objects, all present at time 0. Root `i` starts referencing object `i`; the rest start
    /// unreferenced.
    pub hashes: usize,
    pub roots: usize,
    /// Roots that may be hard-deleted, one of them per exploration.
    pub droppable: u8,
    pub scripts: &'static [&'static [Op]],
    /// Collector runs. The first starts at the first tick; later ones whenever the previous ends.
    pub runs: u8,
    /// The grace period in ticks. Even, at least 4.
    pub grace: u32,
    /// Ticks that the exploration covers.
    pub horizon: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Phase {
    Marking,
    Waiting,
    Deleting,
}

/// `gc/pending.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Pending {
    run: u8,
    phase: Phase,
    cands: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Root {
    alive: bool,
    refs: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Collector {
    Idle,
    /// Phase 1 step 2: about to read root `i`.
    Mark { run: u8, t1: u32, i: u8, seen: u8 },
    /// Phase 1 steps 3–6: listing the objects and publishing the candidates. The listing is one
    /// step: safety does not depend on its being atomic, and splitting it multiplies the states.
    Propose { run: u8, t1: u32, seen: u8 },
    /// Between the phases.
    Wait { run: u8, t1: u32, cands: u8 },
    /// Phase 2 step 2: about to re-read root `i`.
    Recompute { t1: u32, cands: u8, i: u8, seen: u8 },
    /// Phase 2 step 3: about to observe candidate `j` (or the next one after it).
    Observe { t1: u32, cands: u8, refs: u8, j: u8 },
    /// Phase 2 step 3: candidate `j` was observed; `old` if last modified before `t1`.
    Delete { t1: u32, cands: u8, refs: u8, j: u8, old: bool },
    /// Phase 2 step 4.
    Finish,
}

/// A writer's cached copy of the run record, read at `r`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct View {
    r: u32,
    run: Option<Pending>,
}

impl View {
    fn cands(&self) -> u8 {
        self.run.map_or(0, |p| p.cands)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum WriterPc {
    /// About to run the next operation of the script.
    Next,
    /// About to make object `x` safe to reference from root `k`.
    Start { x: u8, k: u8 },
    /// About to upload `x`, relying on the check at `r`. `rescue` is the run it rescues `x` from.
    Put { x: u8, k: u8, r: u32, rescue: Option<u8>, generation: bool },
    /// Rescued `x` from `run`; about to re-read the run record.
    Recheck { x: u8, k: u8, r: u32, run: u8 },
    /// Checked at `r`; about to commit.
    Ready { x: u8, k: u8, r: u32 },
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Writer {
    pc: WriterPc,
    /// The next operation in the script.
    op: u8,
    view: Option<View>,
    /// Flips whenever the view's content changes.
    generation: bool,
    /// Objects checked under §12.4 option 2, renewed by each re-read (the implementation's reuse set).
    reuse: u8,
    /// Objects rescued from the view's run.
    rescued: u8,
    /// Everything written or read, for the unchecked cache.
    known: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct State {
    now: u32,
    /// Each object's modification time, if it exists.
    objects: [Option<u32>; MAX_HASHES],
    roots: [Root; MAX_ROOTS],
    pending: Option<Pending>,
    runs: u8,
    dropped: bool,
    collector: Collector,
    writers: [Writer; MAX_WRITERS],
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Tick,
    Drop(u8),
    Collect,
    Refresh(u8),
    Writer(u8),
}

fn bit(i: u8) -> u8 {
    1 << i
}

/// A writer refreshes its view at most once per tick, and uses a view at most this old.
const VIEW_MAX_AGE: u32 = 1;
/// A writer whose re-reads are further apart than this forgets its checks (renewal, §12.4).
const RENEW_WITHIN: u32 = 1;

struct Model {
    cfg: Config,
    start: u32,
    end: u32,
}

impl Model {
    fn initial(&self) -> State {
        let cfg = &self.cfg;
        let mut objects = [None; MAX_HASHES];
        for o in objects.iter_mut().take(cfg.hashes) {
            *o = Some(0);
        }
        let mut roots = [Root { alive: false, refs: 0 }; MAX_ROOTS];
        for (i, r) in roots.iter_mut().enumerate().take(cfg.roots) {
            *r = Root { alive: true, refs: bit(i as u8) };
        }
        let w = Writer { pc: WriterPc::Next, op: 0, view: None, generation: false, reuse: 0, rescued: 0, known: 0 };
        State { now: self.start, objects, roots, pending: None, runs: 0, dropped: false, collector: Collector::Idle, writers: [w; MAX_WRITERS] }
    }

    /// The safety property: every live root's references exist.
    fn violated(&self, s: &State) -> bool {
        s.roots.iter().take(self.cfg.roots).any(|r| r.alive && (0..self.cfg.hashes).any(|x| r.refs & bit(x as u8) != 0 && s.objects[x].is_none()))
    }

    fn actions(&self, s: &State) -> Vec<Action> {
        let cfg = &self.cfg;
        let mut out = Vec::new();
        if s.now < self.end {
            out.push(Action::Tick);
        }
        if !s.dropped {
            for k in 0..cfg.roots as u8 {
                if cfg.droppable & bit(k) != 0 && s.roots[k as usize].alive {
                    out.push(Action::Drop(k));
                }
            }
        }
        if self.collect(s).is_some() {
            out.push(Action::Collect);
        }
        for (w, wr) in s.writers.iter().enumerate().take(cfg.scripts.len()) {
            if wr.pc != WriterPc::Done && wr.view.is_none_or(|v| s.now > v.r) {
                out.push(Action::Refresh(w as u8));
            }
            if self.writer_step(s, w).is_some() {
                out.push(Action::Writer(w as u8));
            }
        }
        out
    }

    /// The collector's next state, if it can move.
    fn collect(&self, s: &State) -> Option<State> {
        let cfg = &self.cfg;
        let mut n = s.clone();
        n.collector = match s.collector {
            Collector::Idle => {
                if s.pending.is_some() || s.runs >= cfg.runs || (s.runs == 0 && s.now != self.start) {
                    return None;
                }
                let run = s.runs;
                n.runs += 1;
                n.pending = Some(Pending { run, phase: Phase::Marking, cands: 0 });
                Collector::Mark { run, t1: s.now, i: 0, seen: 0 }
            }
            Collector::Mark { run, t1, i, seen } => {
                if (i as usize) < cfg.roots {
                    let r = s.roots[i as usize];
                    Collector::Mark { run, t1, i: i + 1, seen: if r.alive { seen | r.refs } else { seen } }
                } else {
                    Collector::Propose { run, t1, seen }
                }
            }
            Collector::Propose { run, t1, seen } => {
                let old = |j: u8| s.objects[j as usize].is_some_and(|m| m + cfg.grace < t1);
                let cands = (0..cfg.hashes as u8).filter(|&j| seen & bit(j) == 0 && old(j)).fold(0, |c, j| c | bit(j));
                if cands == 0 || (cfg.rules.bound_phase1 && s.now > t1 + cfg.grace / 2) {
                    n.pending = None;
                    Collector::Idle
                } else {
                    n.pending = Some(Pending { run, phase: Phase::Waiting, cands });
                    Collector::Wait { run, t1, cands }
                }
            }
            Collector::Wait { run, t1, cands } => {
                if s.now < t1 + cfg.grace {
                    return None;
                }
                if cfg.rules.mark_deleting {
                    n.pending = Some(Pending { run, phase: Phase::Deleting, cands });
                }
                Collector::Recompute { t1, cands, i: 0, seen: 0 }
            }
            Collector::Recompute { t1, cands, i, seen } => {
                if (i as usize) < cfg.roots {
                    let r = s.roots[i as usize];
                    Collector::Recompute { t1, cands, i: i + 1, seen: if r.alive { seen | r.refs } else { seen } }
                } else {
                    Collector::Observe { t1, cands, refs: seen, j: 0 }
                }
            }
            Collector::Observe { t1, cands, refs, j } => match (j..cfg.hashes as u8).find(|&c| cands & bit(c) != 0) {
                Some(c) => Collector::Delete { t1, cands, refs, j: c, old: s.objects[c as usize].is_some_and(|m| m < t1) },
                None => Collector::Finish,
            },
            Collector::Delete { t1, cands, refs, j, old } => {
                if old && refs & bit(j) == 0 {
                    n.objects[j as usize] = None;
                }
                Collector::Observe { t1, cands, refs, j: j + 1 }
            }
            Collector::Finish => {
                n.pending = None;
                Collector::Idle
            }
        };
        Some(n)
    }

    /// Writer `w`'s next state, if it can move.
    fn writer_step(&self, s: &State, w: usize) -> Option<State> {
        let cfg = &self.cfg;
        let rules = cfg.rules;
        let script = cfg.scripts[w];
        let mut n = s.clone();
        let fresh = |wr: &Writer| wr.view.filter(|v| s.now - v.r <= VIEW_MAX_AGE);
        let wr = &mut n.writers[w];
        wr.pc = match s.writers[w].pc {
            WriterPc::Next => match script.get(wr.op as usize) {
                None => WriterPc::Done,
                Some(&op) => {
                    wr.op += 1;
                    match op {
                        Op::Put { x, k } => WriterPc::Start { x, k },
                        Op::Copy { j, x, k } => {
                            let root = s.roots[j as usize];
                            if root.alive && root.refs & bit(x) != 0 { WriterPc::Ready { x, k, r: s.now } } else { WriterPc::Next }
                        }
                        Op::Read { x } => {
                            if s.objects[x as usize].is_some() {
                                // A GET confirms `x` exists after the view was read (§12.4, 2).
                                if rules.unchecked_cache {
                                    wr.known |= bit(x);
                                }
                                if let Some(v) = fresh(wr)
                                    && v.cands() & bit(x) == 0
                                {
                                    wr.reuse |= bit(x);
                                }
                            }
                            WriterPc::Next
                        }
                    }
                }
            },
            WriterPc::Start { x, k } => {
                if rules.unchecked_cache && wr.known & bit(x) != 0 {
                    WriterPc::Ready { x, k, r: s.now }
                } else if !rules.check_uploads {
                    // Draft 1: an upload confirms itself; reuse still needs a check.
                    match fresh(wr) {
                        Some(v) if wr.reuse & bit(x) != 0 && v.cands() & bit(x) == 0 => WriterPc::Ready { x, k, r: v.r },
                        _ => WriterPc::Put { x, k, r: s.now, rescue: None, generation: wr.generation },
                    }
                } else {
                    let v = fresh(wr)?;
                    match v.run {
                        Some(p) if p.cands & bit(x) != 0 => match p.phase {
                            Phase::Deleting => return None,
                            _ if wr.rescued & bit(x) != 0 => WriterPc::Ready { x, k, r: v.r },
                            _ => WriterPc::Put { x, k, r: v.r, rescue: Some(p.run), generation: wr.generation },
                        },
                        _ if wr.reuse & bit(x) != 0 => WriterPc::Ready { x, k, r: v.r },
                        _ => WriterPc::Put { x, k, r: v.r, rescue: None, generation: wr.generation },
                    }
                }
            }
            WriterPc::Put { x, k, r, rescue, generation } => {
                n.objects[x as usize] = Some(s.now);
                if rules.unchecked_cache {
                    wr.known |= bit(x);
                }
                match rescue {
                    Some(run) if rules.recheck_rescue => WriterPc::Recheck { x, k, r, run },
                    Some(_) => WriterPc::Ready { x, k, r },
                    None => {
                        if wr.generation == generation && wr.view.is_some_and(|v| v.cands() & bit(x) == 0) {
                            wr.reuse |= bit(x);
                        }
                        WriterPc::Ready { x, k, r }
                    }
                }
            }
            WriterPc::Recheck { x, k, r, run } => match s.pending {
                Some(p) if p.run == run && p.phase != Phase::Deleting => {
                    if wr.view.is_some_and(|v| v.run.is_some_and(|vr| vr.run == run)) {
                        wr.rescued |= bit(x);
                    }
                    WriterPc::Ready { x, k, r }
                }
                _ => WriterPc::Start { x, k },
            },
            WriterPc::Ready { x, k, r } => {
                // Commit if still within the bound; otherwise give up (the request fails).
                if s.now < r + cfg.grace / 2 && n.roots[k as usize].alive {
                    n.roots[k as usize].refs |= bit(x);
                }
                WriterPc::Next
            }
            WriterPc::Done => return None,
        };
        Some(n)
    }

    fn apply(&self, s: &State, a: Action) -> State {
        let mut n = s.clone();
        match a {
            Action::Tick => n.now += 1,
            Action::Drop(k) => {
                n.roots[k as usize].alive = false;
                n.dropped = true;
            }
            Action::Collect => return self.collect(s).unwrap(),
            Action::Refresh(w) => {
                let wr = &mut n.writers[w as usize];
                let fresh = View { r: s.now, run: s.pending };
                if wr.view.is_none_or(|v| s.now - v.r > RENEW_WITHIN) {
                    wr.reuse = 0;
                }
                wr.reuse &= !fresh.cands();
                if wr.view.and_then(|v| v.run).map(|p| p.run) != fresh.run.map(|p| p.run) {
                    wr.rescued = 0;
                }
                if wr.view.map(|v| v.run) != Some(fresh.run) {
                    wr.generation = !wr.generation;
                }
                wr.view = Some(fresh);
            }
            Action::Writer(w) => return self.writer_step(s, w as usize).unwrap(),
        }
        n
    }
}

/// What an exploration found.
#[derive(Debug)]
pub enum Outcome {
    /// Every reachable state is safe.
    Safe { states: usize },
    /// A sequence of actions that ends with a live root referencing a missing object.
    Violation { trace: Vec<String> },
    /// The state space is larger than the limit.
    TooBig,
}

struct FxHasher(u64);

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
}

fn fingerprint(s: &State) -> u64 {
    let mut h = FxHasher(0);
    s.hash(&mut h);
    h.finish()
}

/// Explores every interleaving, depth first, up to `limit` distinct states.
pub fn explore(cfg: Config, limit: usize) -> Outcome {
    assert!(cfg.hashes <= MAX_HASHES && cfg.roots <= MAX_ROOTS && cfg.roots <= cfg.hashes && cfg.scripts.len() <= MAX_WRITERS);
    assert!(cfg.grace >= 4 && cfg.grace.is_multiple_of(2));
    // Start once the initial objects are older than the grace period.
    let start = cfg.grace + 1;
    let model = Model { cfg, start, end: start + cfg.horizon };
    let first = model.initial();
    let mut seen: HashSet<u64> = HashSet::new();
    seen.insert(fingerprint(&first));
    let mut stack: Vec<(State, Vec<Action>, usize)> = vec![(first.clone(), model.actions(&first), 0)];
    while let Some((state, actions, next)) = stack.last_mut() {
        let Some(&a) = actions.get(*next) else {
            stack.pop();
            continue;
        };
        *next += 1;
        let child = model.apply(state, a);
        if model.violated(&child) {
            let mut trace: Vec<String> = stack.iter().map(|(s, acts, i)| format!("t={} {:?}", s.now, acts[*i - 1])).collect();
            trace.push(format!("violation at t={}: {:?}", child.now, child));
            return Outcome::Violation { trace };
        }
        if seen.insert(fingerprint(&child)) {
            if seen.len() > limit {
                return Outcome::TooBig;
            }
            let acts = model.actions(&child);
            stack.push((child, acts, 0));
        }
    }
    Outcome::Safe { states: seen.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 5_000_000;

    /// Object 1 is referenced by root 1, which may be hard-deleted; object 2 is garbage from
    /// the start. Writers commit into root 0.
    const REUPLOAD: &[Op] = &[Op::Put { x: 1, k: 0 }];
    const READ_THEN_PUT: &[Op] = &[Op::Read { x: 1 }, Op::Put { x: 1, k: 0 }];
    const COPY: &[Op] = &[Op::Copy { j: 1, x: 1, k: 0 }];
    const PUT_TWICE: &[Op] = &[Op::Put { x: 1, k: 0 }, Op::Put { x: 1, k: 0 }];
    const GARBAGE: &[Op] = &[Op::Put { x: 2, k: 0 }];
    const READ_GARBAGE_THEN_PUT: &[Op] = &[Op::Read { x: 2 }, Op::Put { x: 2, k: 0 }];

    fn config(rules: Rules, scripts: &'static [&'static [Op]], runs: u8) -> Config {
        Config { rules, hashes: 3, roots: 2, droppable: 0b10, scripts, runs, grace: 4, horizon: if runs == 1 { 8 } else { 13 } }
    }

    fn safe(cfg: Config, limit: usize) {
        match explore(cfg, limit) {
            Outcome::Safe { states } => eprintln!("{:?}: safe in {states} states", cfg.scripts),
            Outcome::Violation { trace } => panic!("violation with {:?}:\n{}", cfg.scripts, trace.join("\n")),
            Outcome::TooBig => panic!("state space over {limit} with {:?}", cfg.scripts),
        }
    }

    fn finds_violation(rules: Rules, scripts: &'static [&'static [Op]]) {
        match explore(config(rules, scripts, 1), LIMIT) {
            Outcome::Violation { trace } => eprintln!("{rules:?}:\n{}", trace.join("\n")),
            other => panic!("expected a violation with {rules:?} and {scripts:?}, got {other:?}"),
        }
    }

    const ONE_WRITER: &[&[&[Op]]] = &[&[REUPLOAD], &[READ_THEN_PUT], &[COPY], &[PUT_TWICE], &[GARBAGE], &[READ_GARBAGE_THEN_PUT]];
    const TWO_WRITERS: &[&[&[Op]]] = &[&[REUPLOAD, READ_THEN_PUT], &[COPY, REUPLOAD], &[REUPLOAD, REUPLOAD], &[READ_THEN_PUT, GARBAGE]];

    #[test]
    fn amended_protocol_is_safe_with_one_writer() {
        for &s in ONE_WRITER {
            safe(config(Rules::AMENDED, s, 1), LIMIT);
        }
    }

    /// About a minute in a release build, and 1–2 GB of memory:
    /// `cargo test --release -p voidfs-server gc::model -- --ignored`.
    #[test]
    #[ignore = "slow; run in release"]
    fn amended_protocol_is_safe_with_two_writers() {
        for &s in TWO_WRITERS {
            safe(Config { horizon: 4, ..config(Rules::AMENDED, s, 1) }, 100_000_000);
        }
    }

    /// About two minutes in a release build.
    #[test]
    #[ignore = "slow; run in release"]
    fn amended_protocol_is_safe_across_two_runs() {
        for &s in ONE_WRITER {
            safe(config(Rules::AMENDED, s, 2), 100_000_000);
        }
    }

    #[test]
    fn draft_1_loses_data() {
        finds_violation(Rules::DRAFT1, &[REUPLOAD]);
    }

    #[test]
    fn the_old_cache_loses_data() {
        finds_violation(Rules { unchecked_cache: true, ..Rules::AMENDED }, &[READ_THEN_PUT]);
    }

    #[test]
    fn without_the_deleting_mark_a_rescue_loses_data() {
        finds_violation(Rules { mark_deleting: false, ..Rules::AMENDED }, &[REUPLOAD]);
    }

    #[test]
    fn without_the_recheck_a_rescue_loses_data() {
        finds_violation(Rules { recheck_rescue: false, ..Rules::AMENDED }, &[REUPLOAD]);
    }

    #[test]
    fn unchecked_uploads_lose_data() {
        finds_violation(Rules { check_uploads: false, ..Rules::AMENDED }, &[REUPLOAD]);
    }

    #[test]
    fn an_unbounded_phase_1_loses_data() {
        finds_violation(Rules { bound_phase1: false, ..Rules::AMENDED }, &[READ_THEN_PUT]);
        finds_violation(Rules { bound_phase1: false, ..Rules::AMENDED }, &[COPY]);
    }
}

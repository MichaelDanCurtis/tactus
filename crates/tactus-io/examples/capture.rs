//! Correlate physical controls with HID field indices.
//!
//! The device's report descriptor declares *how many* controls of what width,
//! but not which physical control each index is — it uses a single opaque
//! vendor usage page. The only way to close that gap is to press a control and
//! watch which index moves, so this tool exists to make that session
//! systematic instead of a pile of notes.
//!
//! Two modes:
//!
//! ```text
//! cargo run -p tactus-io --example capture -- watch
//! cargo run -p tactus-io --example capture -- guided names.txt out.tsv
//! ```
//!
//! `watch` prints every field that changes, forever. Use it to confirm the
//! keyboard is reaching this process at all, and to explore.
//!
//! `guided` reads one control name per line, prompts for each in turn, and
//! records which field moved. Press Enter to skip a name that is not on your
//! unit; type `q` to stop early. Results are written as TSV, and the file is
//! flushed after every line so an interrupted session keeps what it learned.
//!
//! # Reading the output
//!
//! A field label is **structural, not semantic** — `R01.bits[12]` means "bit
//! 12 of report 0x01", which is exactly the thing we are trying to give a
//! name. Nothing here invents a name for an index; the names come from the
//! person at the keyboard.
//!
//! # Why consecutive-pair diffing
//!
//! A report carries the state of every control on it, not an event. Pressing
//! one button emits a report where 39 other bits are also present and
//! unchanged. Diffing consecutive reports isolates the field that actually
//! moved; comparing against a fixed zero baseline would light up every control
//! that happens to be resting away from zero.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, Write};
use std::time::{Duration, Instant};

use tactus_hid::{InputReport, decode_input};
use tactus_io::{Device, IoError, MAX_INPUT_REPORT_LEN};

/// One field of one report, flattened to a label and a value.
type Snapshot = BTreeMap<String, u32>;

/// Flatten a report into labelled field values.
///
/// Labels carry the report id so fields from different reports can never be
/// confused for each other — `R01.bits[0]` and `R03.bits[0]` are different
/// controls.
fn flatten(id: u8, report: &InputReport, raw: &[u8]) -> Snapshot {
    let mut out = Snapshot::new();
    let mut put = |label: String, value: u32| {
        out.insert(label, value);
    };
    match report {
        InputReport::Controls(c) => {
            for (i, &b) in c.bits.iter().enumerate() {
                put(format!("R{id:02X}.bits[{i}]"), u32::from(b));
            }
            for (i, &n) in c.nibbles.iter().enumerate() {
                put(format!("R{id:02X}.nibbles[{i}]"), u32::from(n));
            }
        }
        InputReport::Analog(a) => {
            for (i, &b) in a.leading_bits.iter().enumerate() {
                put(format!("R{id:02X}.leading_bits[{i}]"), u32::from(b));
            }
            for (i, &v) in a.continuous.iter().enumerate() {
                put(format!("R{id:02X}.continuous[{i}]"), u32::from(v));
            }
            for (i, &v) in a.wide.iter().enumerate() {
                put(format!("R{id:02X}.wide[{i}]"), u32::from(v));
            }
            for (i, &b) in a.trailing_bits.iter().enumerate() {
                put(format!("R{id:02X}.trailing_bits[{i}]"), u32::from(b));
            }
        }
        InputReport::Wheels(w) => {
            for (i, &b) in w.bits.iter().enumerate() {
                put(format!("R{id:02X}.bits[{i}]"), u32::from(b));
            }
            for (i, &v) in w.full_range.iter().enumerate() {
                put(format!("R{id:02X}.full_range[{i}]"), u32::from(v));
            }
            for (i, &v) in w.eleven_bit.iter().enumerate() {
                put(format!("R{id:02X}.eleven_bit[{i}]"), u32::from(v));
            }
            put(format!("R{id:02X}.tail"), u32::from(w.tail));
        }
        // Unmodelled reports carry no decoded fields, so fall back to raw
        // bytes. 0x40 and 0xAA are the two we understand least, and skipping
        // them here would make the capture session unable to learn anything
        // about exactly the reports that need it most.
        InputReport::Unmodelled { .. } => {
            for (i, &b) in raw.iter().enumerate().skip(1) {
                put(format!("R{id:02X}.raw[{i}]"), u32::from(b));
            }
        }
    }
    out
}

/// What changed between two snapshots of the same report.
fn diff(prev: &Snapshot, next: &Snapshot) -> Vec<(String, u32, u32)> {
    let mut changes = Vec::new();
    for (label, &new) in next {
        let old = prev.get(label).copied().unwrap_or(new);
        if old != new {
            changes.push((label.clone(), old, new));
        }
    }
    changes
}

/// Per-report-id state carried across the whole session.
#[derive(Default)]
struct Tracker {
    last: BTreeMap<u8, Snapshot>,
}

impl Tracker {
    /// Feed a raw report in; return what moved.
    fn observe(&mut self, raw: &[u8]) -> Vec<(String, u32, u32)> {
        let Some(&id) = raw.first() else {
            return Vec::new();
        };
        let decoded = match decode_input(raw) {
            Ok(d) => d,
            // A report we cannot decode is still worth tracking as bytes
            // rather than dropped: a decode failure here is a gap in the SDK,
            // not a reason to stop observing the hardware.
            Err(_) => InputReport::Unmodelled {
                id,
                len: raw.len().saturating_sub(1),
            },
        };
        let snapshot = flatten(id, &decoded, raw);
        let changes = match self.last.get(&id) {
            Some(prev) => diff(prev, &snapshot),
            // First sight of this report id establishes the reference. The
            // report itself is not treated as a change, because we have
            // nothing to compare it against yet.
            None => Vec::new(),
        };
        self.last.insert(id, snapshot);
        changes
    }
}

fn describe(changes: &[(String, u32, u32)]) -> String {
    let mut s = String::new();
    for (label, old, new) in changes {
        let _ = write!(s, "{label} {old}->{new}  ");
    }
    s.trim_end().to_owned()
}

/// Watch forever, printing whatever moves.
fn watch(device: &Device) -> Result<(), IoError> {
    println!("Watching. Press or turn anything. Ctrl-C to stop.\n");
    let mut tracker = Tracker::default();
    let mut buf = [0u8; MAX_INPUT_REPORT_LEN];
    let started = Instant::now();
    let mut seen_ids: BTreeMap<u8, usize> = BTreeMap::new();

    loop {
        let Some(n) = device.poll_raw(Duration::from_millis(200), &mut buf)? else {
            continue;
        };
        let raw = buf.get(..n).unwrap_or(&[]);
        let Some(&id) = raw.first() else { continue };
        *seen_ids.entry(id).or_insert(0) += 1;

        let changes = tracker.observe(raw);
        if changes.is_empty() {
            println!(
                "[{:7.3}s] R{id:02X} ({n} bytes) first sight, reference set",
                started.elapsed().as_secs_f32()
            );
            continue;
        }
        println!(
            "[{:7.3}s] {}",
            started.elapsed().as_secs_f32(),
            describe(&changes)
        );
    }
}

/// Collect every field that moves while a control is being operated.
///
/// Returns the union of changes across the window, most-active field first:
/// turning a knob moves one field many times, while crosstalk (a neighbouring
/// touch sensor, a hand brushing a strip) moves another once or twice. Ranking
/// by activity puts the control you meant at the top without discarding the
/// evidence that something else also moved.
fn capture_window(
    device: &Device,
    tracker: &mut Tracker,
    quiet_for: Duration,
    limit: Duration,
) -> Result<Vec<(String, u32, u32, usize)>, IoError> {
    let mut buf = [0u8; MAX_INPUT_REPORT_LEN];
    let mut tally: BTreeMap<String, (u32, u32, usize)> = BTreeMap::new();
    let started = Instant::now();
    let mut last_activity: Option<Instant> = None;

    loop {
        if started.elapsed() > limit {
            break;
        }
        if last_activity.is_some_and(|t| t.elapsed() > quiet_for) && !tally.is_empty() {
            break;
        }
        let Some(n) = device.poll_raw(Duration::from_millis(50), &mut buf)? else {
            continue;
        };
        let raw = buf.get(..n).unwrap_or(&[]);
        for (label, old, new) in tracker.observe(raw) {
            last_activity = Some(Instant::now());
            tally
                .entry(label)
                .and_modify(|e| {
                    e.1 = new;
                    e.2 += 1;
                })
                .or_insert((old, new, 1));
        }
    }

    let mut ranked: Vec<(String, u32, u32, usize)> = tally
        .into_iter()
        .map(|(label, (old, new, count))| (label, old, new, count))
        .collect();
    ranked.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));
    Ok(ranked)
}

/// Prompt through a list of control names, recording what each one moves.
fn guided(device: &Device, names_path: &str, out_path: &str) -> Result<(), IoError> {
    let names_text = match fs::read_to_string(names_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {names_path}: {e}");
            return Ok(());
        }
    };
    let names: Vec<&str> = names_text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let mut out = match fs::File::create(out_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot write {out_path}: {e}");
            return Ok(());
        }
    };
    let _ = writeln!(out, "# control\tfield\tfrom\tto\tsamples\talternates");

    println!("\n{} control(s) to capture.\n", names.len());
    println!("For each prompt: operate the control, then stop.");
    println!("  - buttons: press AND release");
    println!("  - knobs/encoders: turn a little");
    println!("  - strips/pedals: sweep through the range");
    println!("Enter alone = skip (not on this unit). 'q' = stop. 'r' = redo last.\n");

    let mut tracker = Tracker::default();
    let mut buf = [0u8; MAX_INPUT_REPORT_LEN];

    // Drain anything already queued, and let the tracker learn each report's
    // resting state, so the first real capture is not polluted by a baseline
    // being established mid-window.
    println!("Settling (don't touch anything for 2s)...");
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(2) {
        if let Some(n) = device.poll_raw(Duration::from_millis(100), &mut buf)? {
            let raw = buf.get(..n).unwrap_or(&[]);
            tracker.observe(raw);
        }
    }
    println!("Ready.\n");

    let stdin = std::io::stdin();
    let mut recorded = 0usize;
    let mut index = 0usize;

    while index < names.len() {
        let name = names.get(index).copied().unwrap_or("");
        print!("[{}/{}] {name}  -> ", index + 1, names.len());
        let _ = std::io::stdout().flush();

        let mut line = String::new();
        if stdin.lock().read_line(&mut line).is_err() {
            break;
        }
        let answer = line.trim();
        if answer.eq_ignore_ascii_case("q") {
            println!("stopping early");
            break;
        }
        if answer.eq_ignore_ascii_case("r") {
            index = index.saturating_sub(1);
            continue;
        }
        if !answer.is_empty() {
            // Anything else typed is treated as a skip with a note, so a user
            // can say why without derailing the session.
            println!("    skipped: {answer}");
            index += 1;
            continue;
        }

        let changes = capture_window(
            device,
            &mut tracker,
            Duration::from_millis(400),
            Duration::from_secs(6),
        )?;

        if changes.is_empty() {
            println!("    nothing moved — skipped");
            index += 1;
            continue;
        }

        let Some((field, from, to, samples)) = changes.first().cloned() else {
            index += 1;
            continue;
        };
        let alternates: Vec<String> = changes
            .iter()
            .skip(1)
            .map(|(l, _, _, c)| format!("{l}({c})"))
            .collect();

        println!("    {field}  {from}->{to}  [{samples} samples]");
        if !alternates.is_empty() {
            println!("    also moved: {}", alternates.join(" "));
        }

        let _ = writeln!(
            out,
            "{name}\t{field}\t{from}\t{to}\t{samples}\t{}",
            alternates.join(" ")
        );
        let _ = out.flush();
        recorded += 1;
        index += 1;
    }

    println!("\nrecorded {recorded} mapping(s) to {out_path}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("watch");

    // Validate the mode *before* opening anything, so `--help` on a machine
    // with no keyboard prints usage instead of "could not open a keyboard".
    let guided_args = match mode {
        "watch" => None,
        "guided" => match args.get(1) {
            Some(names) => Some((
                names.clone(),
                args.get(2)
                    .cloned()
                    .unwrap_or_else(|| "capture.tsv".to_owned()),
            )),
            None => {
                eprintln!("usage: capture guided <names.txt> [out.tsv]");
                std::process::exit(2);
            }
        },
        other => {
            // Previously any unrecognised argument fell through to `watch`,
            // which loops forever: `capture --help` hung instead of answering.
            eprintln!("unknown mode {other:?}");
            eprintln!("usage: capture watch");
            eprintln!("       capture guided <names.txt> [out.tsv]");
            std::process::exit(2);
        }
    };

    let device = match Device::open_only() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not open a keyboard: {e}");
            std::process::exit(1);
        }
    };
    println!("opened {:?}", device.model());
    match device.is_open_exclusive() {
        Ok(true) => eprintln!("WARNING: opened exclusively — other software has lost the device"),
        Ok(false) => println!("sharing the device (not seizing it)"),
        Err(e) => eprintln!("could not check exclusivity: {e}"),
    }

    let result = match guided_args {
        Some((names, out)) => guided(&device, &names, &out),
        None => watch(&device),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

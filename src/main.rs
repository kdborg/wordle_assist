// wordle [n]
//
// An interactive command-line Wordle *solver*. It keeps the master list of TWL
// words of the chosen length and narrows it down as you feed in the clues
// Wordle gives you.
//
// Usage:
//   wordle [n]     n = word length, 2..=15, default 5
//
// At the "Guess:" prompt you can enter:
//   q                quit
//   n                reset — forget all clues, restore the full candidate list
//   n <k>            reset AND switch to k-letter words (k = 2..=15)
//   word [letters]   a guess and its result
//
// The guess result is encoded in the casing of `word`, plus an optional group
// of letters after it:
//   * A CAPITAL letter  -> correct letter, correct position (green). That
//     position is now pinned to exactly that one letter.
//   * Any letters typed after the word -> present in the answer but in the
//     WRONG place (yellow); they are barred from the position(s) they occupy in
//     `word`.
//   * Every other (lowercase) letter of `word` -> not in the answer (gray).
//
// Example: answer "CRANE", you guess "trace" — 'r','a' green, 'e' yellow,
// 't','c' gray:   tRAce e
//
// After each guess we print the alphabetized list of words still possible, the
// letters still possible at each position, and how many remain out of the total
// for this word length.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The word list. We use an absolute path so the program works no matter which
/// directory it is launched from.
const DICT_ABS: &str = "/Users/kirkbrown/bin/scrabble_dictionary/twl.txt";

/// Relative form, used only as a fallback if the absolute path is missing (e.g.
/// the tree was moved). Resolved against the cwd and the executable's location.
const DICT_REL: &str = "scrabble_dictionary/twl.txt";

/// Locate the word list: the absolute path first, then a best-effort search.
fn dict_path() -> Option<PathBuf> {
    let abs = PathBuf::from(DICT_ABS);
    if abs.is_file() {
        return Some(abs);
    }
    find_dict()
}

/// Find the word list by trying a handful of likely locations: relative to the
/// current directory and relative to the executable, walking a few parents up.
fn find_dict() -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            roots.push(dir.to_path_buf());
        }
    }
    for root in roots {
        let mut dir = root.as_path();
        for _ in 0..5 {
            let candidate = dir.join(DICT_REL);
            if candidate.is_file() {
                return Some(candidate);
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
    }
    None
}

/// Index 0..=25 for an ASCII letter (either case).
fn idx(c: u8) -> usize {
    (c.to_ascii_uppercase() - b'A') as usize
}

/// Per-letter occurrence counts for an (uppercase) word.
fn counts_of(word: &[u8]) -> [usize; 26] {
    let mut c = [0usize; 26];
    for &b in word {
        c[idx(b)] += 1;
    }
    c
}

/// Everything we've learned since the last reset.
struct Solver {
    dict: PathBuf,
    len: usize,
    master: Vec<String>,  // all words of this length, uppercase, alphabetized
    current: Vec<String>, // those still consistent with the clues
    in_letters: [bool; 26],
    out_letters: [bool; 26],
    /// For each position, letters that cannot go there.
    wrong_positions: Vec<[bool; 26]>,
    /// For each position, the pinned (green) letter, if known.
    correct_positions: Vec<Option<u8>>,
    /// Minimum number of times each letter must appear.
    total_counts: [usize; 26],
}

impl Solver {
    fn new(dict: PathBuf, len: usize, master: Vec<String>) -> Self {
        let mut s = Solver {
            dict,
            len,
            current: master.clone(),
            master,
            in_letters: [false; 26],
            out_letters: [false; 26],
            wrong_positions: vec![[false; 26]; len],
            correct_positions: vec![None; len],
            total_counts: [0; 26],
        };
        s.reset();
        s
    }

    /// Forget every clue and restore the full candidate list.
    fn reset(&mut self) {
        self.in_letters = [false; 26];
        self.out_letters = [false; 26];
        self.wrong_positions = vec![[false; 26]; self.len];
        self.correct_positions = vec![None; self.len];
        self.total_counts = [0; 26];
        self.current = self.master.clone();
    }

    /// Switch to a different word length: reload the master list, then reset.
    fn set_length(&mut self, new_len: usize) -> Result<(), String> {
        let master = load_master(&self.dict, new_len)
            .map_err(|e| format!("cannot read word list: {e}"))?;
        if master.is_empty() {
            return Err(format!("no {new_len}-letter words found in the word list"));
        }
        self.len = new_len;
        self.master = master;
        self.reset();
        Ok(())
    }

    /// Check a guess line before applying it. Returns Err(reason) if unusable.
    fn validate(&self, guess: &[u8], wrongs: &[u8]) -> Result<(), String> {
        if guess.len() != self.len {
            return Err(format!(
                "guess has {} letters, expected {}",
                guess.len(),
                self.len
            ));
        }
        if !guess.iter().all(|b| b.is_ascii_alphabetic()) {
            return Err("guess must be letters only".into());
        }

        // A new green must not contradict a previously established one.
        for i in 0..self.len {
            if guess[i].is_ascii_uppercase() {
                if let Some(prev) = self.correct_positions[i] {
                    if prev != guess[i] {
                        return Err(format!(
                            "position {} was already fixed to '{}'",
                            i + 1,
                            prev as char
                        ));
                    }
                }
            }
        }

        if wrongs.len() > self.len {
            return Err("too many wrong-position letters".into());
        }
        for &c in wrongs {
            if !c.is_ascii_alphabetic() {
                return Err("wrong-position list must be letters only".into());
            }
            // The letter must appear as a lowercase (non-green) letter in the
            // guess — you can't call a letter yellow if it isn't there.
            if !guess.iter().any(|&g| g == c.to_ascii_lowercase()) {
                return Err(format!("wrong-position letter '{}' is not in the guess", c as char));
            }
            if self.out_letters[idx(c)] {
                return Err(format!("'{}' was previously marked absent", c as char));
            }
        }
        Ok(())
    }

    /// Fold one validated guess into the accumulated clues and re-filter.
    fn apply(&mut self, guess: &[u8], wrongs: &[u8]) {
        // Minimum counts implied by this guess: yellows first, then a bump per
        // green as we walk the word.
        let mut this_counts = counts_of(wrongs);

        for i in 0..self.len {
            let c = guess[i];
            let up = c.to_ascii_uppercase();
            let k = idx(c);

            if c.is_ascii_uppercase() {
                // Green.
                this_counts[k] += 1;
                self.correct_positions[i] = Some(up);
                self.wrong_positions[i][k] = false;
                self.in_letters[k] = true;
            } else if wrongs.iter().any(|&w| w.to_ascii_uppercase() == up) {
                // Yellow: present, but not here.
                self.in_letters[k] = true;
                self.wrong_positions[i][k] = true;
            } else {
                // Gray: absent.
                self.out_letters[k] = true;
            }
        }

        // A letter that's required somewhere can't also be "absent".
        for k in 0..26 {
            if self.in_letters[k] {
                self.out_letters[k] = false;
            }
        }

        // Raise the running minimum counts.
        for k in 0..26 {
            if this_counts[k] > self.total_counts[k] {
                self.total_counts[k] = this_counts[k];
            }
        }

        // The exact guessed word (uppercased) is never the answer once scored.
        let guessed: Vec<u8> = guess.iter().map(|b| b.to_ascii_uppercase()).collect();

        self.current.retain(|word| {
            let w = word.as_bytes();
            let wc = counts_of(w);

            for k in 0..26 {
                if self.out_letters[k] && wc[k] > 0 {
                    return false; // contains an absent letter
                }
                if self.in_letters[k] && wc[k] == 0 {
                    return false; // missing a required letter
                }
                if wc[k] < self.total_counts[k] {
                    return false; // not enough copies of a letter
                }
            }

            for pos in 0..self.len {
                if let Some(g) = self.correct_positions[pos] {
                    if w[pos] != g {
                        return false; // green letter not in place
                    }
                }
                if self.wrong_positions[pos][idx(w[pos])] {
                    return false; // a yellow letter sitting in a barred spot
                }
            }

            w != guessed.as_slice()
        });
    }

    /// Print the alphabetized survivors, the letters still possible at each
    /// position, and the running tally.
    fn report(&self) {
        for w in &self.current {
            println!("{w}");
        }

        if !self.current.is_empty() {
            println!("Available letters by position:");
            for pos in 0..self.len {
                // The distinct letters that actually occur at this position
                // across every remaining candidate.
                let mut seen = [false; 26];
                for w in &self.current {
                    seen[idx(w.as_bytes()[pos])] = true;
                }
                let letters: String = (0..26)
                    .filter(|&k| seen[k])
                    .map(|k| (b'A' + k as u8) as char)
                    .collect();
                println!("  {:>2}: {}", pos + 1, letters);
            }
        }

        println!(
            "{} of {} {}-letter words remain",
            self.current.len(),
            self.master.len(),
            self.len
        );
    }
}

fn load_master(dict: &Path, len: usize) -> io::Result<Vec<String>> {
    let raw = std::fs::read_to_string(dict)?;
    let mut words: Vec<String> = raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.len() == len && l.bytes().all(|b| b.is_ascii_alphabetic()))
        .map(|l| l.to_ascii_uppercase())
        .collect();
    words.sort();
    words.dedup();
    Ok(words)
}

/// Restore the default SIGPIPE disposition so that piping our output into a
/// reader that closes early (e.g. `wordle | head`) terminates us quietly
/// instead of panicking on a broken-pipe write. Rust ignores SIGPIPE by
/// default; standard Unix tools do not.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}
#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() -> ExitCode {
    reset_sigpipe();

    // ---- word length argument ----
    let args: Vec<String> = std::env::args().collect();
    let len: usize = match args.get(1) {
        None => 5,
        Some(a) => match a.parse::<usize>() {
            Ok(n) if (2..=15).contains(&n) => n,
            _ => {
                eprintln!("usage: wordle [n]   where n is 2..=15 (default 5)");
                return ExitCode::FAILURE;
            }
        },
    };

    // ---- load the dictionary ----
    let dict = match dict_path() {
        Some(p) => p,
        None => {
            eprintln!("cannot find word list at {DICT_ABS} (or {DICT_REL} near the cwd/executable)");
            return ExitCode::FAILURE;
        }
    };
    let master = match load_master(&dict, len) {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => {
            eprintln!("no {len}-letter words found in the word list");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("cannot read {}: {e}", dict.display());
            return ExitCode::FAILURE;
        }
    };

    println!(
        "Wordle solver — {}-letter words, {} candidates.",
        len,
        master.len()
    );
    println!("At the prompt: 'q' quit, 'n' reset, 'n <k>' reset to k letters, or  word [letters]");
    println!("  (CAPITALS = right spot, letters after the word = wrong spot.)");

    let mut solver = Solver::new(dict, len, master);

    let stdin = io::stdin();
    loop {
        print!("Guess: ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break, // EOF / Ctrl-D
            Ok(_) => {}
            Err(e) => {
                eprintln!("input error: {e}");
                break;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line == "q" {
            break;
        }

        // Reset commands: "n" alone, or "n <k>" to also change the word length.
        let mut tokens = line.split_whitespace();
        let first = tokens.next().unwrap();
        if first == "n" {
            match tokens.next() {
                None => {
                    solver.reset();
                    println!("Reset. {} candidates.", solver.current.len());
                }
                Some(num) => match num.parse::<usize>() {
                    Ok(k) if (2..=15).contains(&k) => match solver.set_length(k) {
                        Ok(()) => println!(
                            "Reset to {}-letter words. {} candidates.",
                            k,
                            solver.current.len()
                        ),
                        Err(e) => println!("? {e}"),
                    },
                    _ => println!("? new word length must be a number 2..=15"),
                },
            }
            continue;
        }

        // Otherwise: a guess line.
        let mut parts = line.split_whitespace();
        let guess = parts.next().unwrap().as_bytes().to_vec();
        let wrongs: Vec<u8> = parts.flat_map(|t| t.bytes()).collect();

        match solver.validate(&guess, &wrongs) {
            Ok(()) => {
                solver.apply(&guess, &wrongs);
                solver.report();
            }
            Err(msg) => println!("? {msg}"),
        }
    }

    println!("bye");
    ExitCode::SUCCESS
}

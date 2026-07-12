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
//   * Any letters typed after the word -> present in the answer but in the WRONG
//     place (yellow). Repeat a letter once per yellow copy: "eerie ee" says two
//     of that guess's three e's came up yellow.
//   * Every other (lowercase) letter of `word` -> gray.
//
// A lowercase letter is never at the position it occupies in `word` — yellow or
// gray, it would have been a capital if it were. And because Wordle only greys a
// copy of a letter once it has run out of them, a single gray copy pins the
// letter's total: the answer holds exactly as many as the guess just proved green
// or yellow. Zero for a letter that never lit up; exactly one L for "lAbEL" (one
// green L, one gray); exactly two e's for "eerie ee" (two yellow, one gray).
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
    /// For each position, letters that cannot go there.
    wrong_positions: Vec<[bool; 26]>,
    /// For each position, the pinned (green) letter, if known.
    correct_positions: Vec<Option<u8>>,
    /// Minimum number of times each letter must appear.
    total_counts: [usize; 26],
    /// Maximum number of times each letter may appear. A gray letter drives this
    /// down; 0 means the letter is absent from the answer entirely.
    max_counts: [usize; 26],
}

impl Solver {
    fn new(dict: PathBuf, len: usize, master: Vec<String>) -> Self {
        let mut s = Solver {
            dict,
            len,
            current: master.clone(),
            master,
            wrong_positions: vec![[false; 26]; len],
            correct_positions: vec![None; len],
            total_counts: [0; 26],
            max_counts: [len; 26],
        };
        s.reset();
        s
    }

    /// Forget every clue and restore the full candidate list.
    fn reset(&mut self) {
        self.wrong_positions = vec![[false; 26]; self.len];
        self.correct_positions = vec![None; self.len];
        self.total_counts = [0; 26];
        self.max_counts = [self.len; 26];
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

        // A new green must not contradict a previously established one, and a
        // known green must not be typed in lowercase — that would read as gray
        // and rule out the very letter we already know sits there.
        for i in 0..self.len {
            if let Some(prev) = self.correct_positions[i] {
                if guess[i].is_ascii_uppercase() && prev != guess[i] {
                    return Err(format!(
                        "position {} was already fixed to '{}'",
                        i + 1,
                        prev as char
                    ));
                }
                if guess[i].is_ascii_lowercase() && prev == guess[i].to_ascii_uppercase() {
                    return Err(format!(
                        "position {} is known to be '{}' — capitalize it",
                        i + 1,
                        prev as char
                    ));
                }
            }
        }

        if wrongs.len() > self.len {
            return Err("too many wrong-position letters".into());
        }
        if !wrongs.iter().all(|b| b.is_ascii_alphabetic()) {
            return Err("wrong-position list must be letters only".into());
        }

        // A yellow has to be an actual non-green letter of the guess, and it
        // can't be claimed more often than the guess has copies left to claim —
        // "eerie ee" is two of the three e's, "eerie eee" would be all of them,
        // and "arose aa" is nonsense.
        let listed = counts_of(wrongs);
        let mut ungreen = [0usize; 26];
        for &g in guess {
            if g.is_ascii_lowercase() {
                ungreen[idx(g)] += 1;
            }
        }
        for k in 0..26 {
            if listed[k] == 0 {
                continue;
            }
            let c = (b'A' + k as u8) as char;
            if ungreen[k] == 0 {
                return Err(format!("wrong-position letter '{c}' is not in the guess"));
            }
            if listed[k] > ungreen[k] {
                return Err(format!(
                    "'{c}' is marked wrong-position {} times but the guess has only {} of it outside the capitals",
                    listed[k], ungreen[k]
                ));
            }
            if self.max_counts[k] == 0 {
                return Err(format!("'{c}' was previously marked absent"));
            }
        }
        Ok(())
    }

    /// Fold one validated guess into the accumulated clues and re-filter.
    fn apply(&mut self, guess: &[u8], wrongs: &[u8]) {
        // Per letter: how many copies the guess spends, and how many of those the
        // clue proved are in the answer — one per green, one per yellow (a letter
        // repeated in `wrongs` counts once per repeat).
        let spent = counts_of(guess);
        let mut proved = counts_of(wrongs);
        for i in 0..self.len {
            if guess[i].is_ascii_uppercase() {
                proved[idx(guess[i])] += 1;
            }
        }

        for i in 0..self.len {
            let c = guess[i];
            let k = idx(c);
            if c.is_ascii_uppercase() {
                // Green: this position is pinned to this letter.
                self.correct_positions[i] = Some(c);
                self.wrong_positions[i][k] = false;
            } else {
                // Not green, so — yellow or gray alike — the answer does not have
                // this letter *here*; it would have shown green if it did.
                self.wrong_positions[i][k] = true;
            }
        }

        for k in 0..26 {
            // Every copy the guess spent without proving it is a gray copy, and a
            // gray copy caps the letter: Wordle only greys a copy once it has run
            // out of them, so the answer holds exactly `proved[k]`. Zero for a
            // letter that never lit up, one for the L of "lAbEL", two for the e's
            // of "eerie ee".
            if spent[k] > proved[k] && proved[k] < self.max_counts[k] {
                self.max_counts[k] = proved[k];
            }
            // Raise the running minimum count.
            if proved[k] > self.total_counts[k] {
                self.total_counts[k] = proved[k];
            }
        }

        // The exact guessed word (uppercased) is never the answer once scored.
        let guessed: Vec<u8> = guess.iter().map(|b| b.to_ascii_uppercase()).collect();

        self.current.retain(|word| {
            let w = word.as_bytes();
            let wc = counts_of(w);

            for k in 0..26 {
                if wc[k] < self.total_counts[k] {
                    return false; // not enough copies of a letter
                }
                if wc[k] > self.max_counts[k] {
                    return false; // too many copies (0 = an absent letter)
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

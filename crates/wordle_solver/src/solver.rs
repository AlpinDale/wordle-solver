use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashMap;

use crate::corpus::Corpus;
use crate::{Feedback, SolverError, Word, FEEDBACK_STATES};

static NEXT_GUESS_CACHE: OnceLock<Mutex<FxHashMap<Vec<u16>, usize>>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct SolveStep {
    pub guess: Word,
    pub feedback: Feedback,
    pub remaining_answers: usize,
}

#[derive(Clone, Debug)]
pub struct SolveTrace {
    pub answer: Word,
    pub steps: Vec<SolveStep>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SolverStatus {
    InProgress,
    Solved(Word),
}

#[derive(Debug)]
pub struct OfficialSolver {
    remaining: Box<[u64]>,
    remaining_answers: Vec<u16>,
    remaining_answer_guess_flags: Box<[u8]>,
    remaining_count: usize,
    pending_guess: Option<usize>,
    cached_guess: Option<usize>,
    solved_word: Option<Word>,
}

impl Default for OfficialSolver {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GuessScore {
    worst_bucket: u16,
    expected_bucket_sum: u32,
    prefer_answer: bool,
    guess_index: usize,
}

struct GuessScoringScratch {
    histogram: [u16; FEEDBACK_STATES],
    epochs: [u16; FEEDBACK_STATES],
    epoch: u16,
}

impl GuessScoringScratch {
    fn new() -> Self {
        Self {
            histogram: [0_u16; FEEDBACK_STATES],
            epochs: [0_u16; FEEDBACK_STATES],
            epoch: 1,
        }
    }
}

impl GuessScore {
    fn better_than(self, other: Self) -> bool {
        (
            self.worst_bucket,
            self.expected_bucket_sum,
            !self.prefer_answer,
            self.guess_index,
        ) < (
            other.worst_bucket,
            other.expected_bucket_sum,
            !other.prefer_answer,
            other.guess_index,
        )
    }
}

impl OfficialSolver {
    pub fn try_new() -> Result<Self, SolverError> {
        let corpus = Corpus::load()?;
        let mut remaining = vec![u64::MAX; corpus.answer_count().div_ceil(64)].into_boxed_slice();
        let trailing = corpus.answer_count() % 64;
        if trailing != 0 {
            let last_index = remaining.len() - 1;
            remaining[last_index] = (1_u64 << trailing) - 1;
        }
        let remaining_answers = (0..corpus.answer_count())
            .map(|answer_index| answer_index as u16)
            .collect();
        let mut remaining_answer_guess_flags = vec![0_u8; corpus.guess_count()].into_boxed_slice();
        for answer_index in 0..corpus.answer_count() {
            remaining_answer_guess_flags[corpus.answer_guess_index(answer_index)] = 1;
        }

        Ok(Self {
            remaining,
            remaining_answers,
            remaining_answer_guess_flags,
            remaining_count: corpus.answer_count(),
            pending_guess: None,
            cached_guess: Some(corpus.first_guess_index()),
            solved_word: None,
        })
    }

    pub fn new() -> Self {
        match Self::try_new() {
            Ok(solver) => solver,
            Err(error) => panic!("official bundle should be present: {error}"),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn remaining_answers(&self) -> usize {
        self.remaining_count
    }

    pub fn remaining_candidates(&self) -> Result<Vec<Word>, SolverError> {
        let corpus = Corpus::load()?;
        Ok(self
            .remaining_answers
            .iter()
            .copied()
            .map(|answer_index| corpus.answer_word(answer_index as usize))
            .collect())
    }

    pub fn next_guess(&mut self) -> Word {
        if let Some(word) = self.solved_word {
            return word;
        }

        let corpus = match Corpus::load() {
            Ok(corpus) => corpus,
            Err(error) => panic!("official bundle should be present: {error}"),
        };
        let guess_index = self.cached_guess.unwrap_or_else(|| {
            let cache = NEXT_GUESS_CACHE.get_or_init(|| Mutex::new(FxHashMap::default()));
            if let Some(&guess_index) = cache
                .lock()
                .expect("next-guess cache lock should not be poisoned")
                .get(self.remaining_answers.as_slice())
            {
                return guess_index;
            }

            let guess_index = self.compute_best_guess(corpus);
            cache
                .lock()
                .expect("next-guess cache lock should not be poisoned")
                .insert(self.remaining_answers.clone(), guess_index);
            guess_index
        });
        self.issue_guess_index(guess_index, corpus)
    }

    pub fn pending_guess(&self) -> Result<Option<Word>, SolverError> {
        let Some(guess_index) = self.pending_guess else {
            return Ok(None);
        };
        Ok(Some(Corpus::load()?.guess_word(guess_index)))
    }

    pub fn issue_guess(&mut self, guess: Word) -> Result<Word, SolverError> {
        let corpus = Corpus::load()?;
        let guess_index = corpus.find_guess(guess).ok_or(SolverError::UnknownGuess)?;
        Ok(self.issue_guess_index(guess_index, corpus))
    }

    pub fn apply_feedback(&mut self, feedback: Feedback) -> Result<SolverStatus, SolverError> {
        if self.solved_word.is_some() {
            return Err(SolverError::AlreadySolved);
        }

        let corpus = Corpus::load()?;
        let guess_index = self
            .pending_guess
            .take()
            .ok_or(SolverError::GuessNotIssued)?;

        let mut next_remaining = vec![0_u64; self.remaining.len()].into_boxed_slice();
        let mut next_answers = Vec::with_capacity(self.remaining_answers.len());
        let feedback_row = corpus.feedback_row(guess_index);
        let feedback_code = feedback.code();

        for &answer_index in &self.remaining_answers {
            let answer_index = answer_index as usize;
            if feedback_row[answer_index] == feedback_code {
                set_bit(&mut next_remaining, answer_index, true);
                next_answers.push(answer_index as u16);
            }
        }

        if next_answers.is_empty() {
            return Err(SolverError::Contradiction);
        }

        let mut next_flags = vec![0_u8; corpus.guess_count()].into_boxed_slice();
        for &answer_index in &next_answers {
            next_flags[corpus.answer_guess_index(answer_index as usize)] = 1;
        }

        self.remaining = next_remaining;
        self.remaining_count = next_answers.len();
        self.remaining_answers = next_answers;
        self.remaining_answer_guess_flags = next_flags;
        self.cached_guess = None;

        if feedback.is_solved() {
            let solved_word = corpus.guess_word(guess_index);
            self.solved_word = Some(solved_word);
            return Ok(SolverStatus::Solved(solved_word));
        }

        Ok(SolverStatus::InProgress)
    }

    pub fn simulate(answer: Word) -> Result<SolveTrace, SolverError> {
        let corpus = Corpus::load()?;
        let answer_index = corpus
            .find_answer(answer)
            .ok_or(SolverError::UnknownAnswer)?;
        let mut solver = Self::try_new()?;
        let mut steps = Vec::new();

        loop {
            let guess = solver.next_guess();
            let guess_index = corpus.find_guess(guess).ok_or(SolverError::AssetCorrupt)?;
            let feedback = corpus.feedback(guess_index, answer_index);
            let status = solver.apply_feedback(feedback)?;
            steps.push(SolveStep {
                guess,
                feedback,
                remaining_answers: solver.remaining_answers(),
            });

            if let SolverStatus::Solved(_) = status {
                return Ok(SolveTrace { answer, steps });
            }
        }
    }

    fn compute_best_guess(&self, corpus: &Corpus) -> usize {
        if self.remaining_count == 1 {
            let Some(&answer_index) = self.remaining_answers.first() else {
                debug_assert!(
                    false,
                    "single-answer state should have one remaining answer"
                );
                return corpus.first_guess_index();
            };
            return corpus.answer_guess_index(answer_index as usize);
        }

        let mut best = GuessScore {
            worst_bucket: u16::MAX,
            expected_bucket_sum: u32::MAX,
            prefer_answer: false,
            guess_index: usize::MAX,
        };

        let mut scratch = GuessScoringScratch::new();
        for &answer_index in &self.remaining_answers {
            let guess_index = corpus.answer_guess_index(answer_index as usize);
            if let Some(score) =
                self.score_guess_index(corpus, guess_index, true, best, &mut scratch)
            {
                if score.better_than(best) {
                    best = score;
                }
            }
        }

        for guess_index in 0..corpus.guess_count() {
            if self.remaining_answer_guess_flags[guess_index] != 0 {
                continue;
            }
            if let Some(score) = self.score_guess_index(
                corpus,
                guess_index,
                false,
                best,
                &mut scratch,
            ) {
                if score.better_than(best) {
                    best = score;
                }
            }
        }

        best.guess_index
    }

    #[inline(always)]
    fn score_guess_index(
        &self,
        corpus: &Corpus,
        guess_index: usize,
        prefer_answer: bool,
        best: GuessScore,
        scratch: &mut GuessScoringScratch,
    ) -> Option<GuessScore> {
        scratch.epoch = scratch.epoch.wrapping_add(1);
        if scratch.epoch == 0 {
            scratch.epochs.fill(0);
            scratch.epoch = 1;
        }

        let mut worst_bucket = 0_u16;
        let mut expected_bucket_sum = 0_u32;
        let feedback_row = corpus.feedback_row(guess_index);

        for &answer_index in &self.remaining_answers {
            let bucket_index = feedback_row[answer_index as usize] as usize;
            let previous = if scratch.epochs[bucket_index] == scratch.epoch {
                scratch.histogram[bucket_index]
            } else {
                scratch.epochs[bucket_index] = scratch.epoch;
                scratch.histogram[bucket_index] = 0;
                0
            };

            let next = previous + 1;
            scratch.histogram[bucket_index] = next;
            expected_bucket_sum += u32::from(previous) * 2 + 1;
            worst_bucket = worst_bucket.max(next);

            if worst_bucket > best.worst_bucket
                || (worst_bucket == best.worst_bucket
                    && expected_bucket_sum > best.expected_bucket_sum)
            {
                return None;
            }
        }

        Some(GuessScore {
            worst_bucket,
            expected_bucket_sum,
            prefer_answer,
            guess_index,
        })
    }

    fn issue_guess_index(&mut self, guess_index: usize, corpus: &Corpus) -> Word {
        self.pending_guess = Some(guess_index);
        corpus.guess_word(guess_index)
    }
}

fn set_bit(words: &mut [u64], index: usize, enabled: bool) {
    let mask = 1_u64 << (index % 64);
    let slot = &mut words[index / 64];
    if enabled {
        *slot |= mask;
    } else {
        *slot &= !mask;
    }
}

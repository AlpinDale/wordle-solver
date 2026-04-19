use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::corpus::Corpus;
use crate::{Feedback, SolverError, Word, FEEDBACK_STATES};

thread_local! {
    static NEXT_GUESS_CACHE: RefCell<FxHashMap<StateKey, Vec<CacheEntry>>> = RefCell::new(FxHashMap::default());
}
static OPENING_RESPONSE_CACHE: std::sync::OnceLock<Box<[usize; FEEDBACK_STATES]>> =
    std::sync::OnceLock::new();
static THIRD_TURN_CACHE: std::sync::OnceLock<FxHashMap<StateKey, usize>> =
    std::sync::OnceLock::new();
static FOURTH_TURN_CACHE: std::sync::OnceLock<FxHashMap<StateKey, usize>> =
    std::sync::OnceLock::new();

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
    remaining_answers: Vec<u16>,
    scratch_answers: Vec<u16>,
    remaining_count: usize,
    state_hash: u64,
    turns_taken: u8,
    opening_feedback: Option<u8>,
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct StateKey {
    remaining_count: u16,
    state_hash: u64,
}

#[derive(Clone, Debug)]
struct CacheEntry {
    answers: Box<[u16]>,
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
        let remaining_answers = (0..corpus.answer_count())
            .map(|answer_index| answer_index as u16)
            .collect();
        let scratch_answers = Vec::with_capacity(corpus.answer_count());

        Ok(Self {
            remaining_answers,
            scratch_answers,
            remaining_count: corpus.answer_count(),
            state_hash: corpus.initial_state_hash(),
            turns_taken: 0,
            opening_feedback: None,
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
            if self.turns_taken == 1 {
                if let Some(feedback_code) = self.opening_feedback {
                    return opening_response_guess(corpus, feedback_code);
                }
            }
            if self.turns_taken == 2 {
                let cache_key = StateKey {
                    remaining_count: self.remaining_count as u16,
                    state_hash: self.state_hash,
                };
                if let Some(&guess_index) = third_turn_cache(corpus).get(&cache_key) {
                    return guess_index;
                }
            }
            if self.turns_taken == 3 {
                let cache_key = StateKey {
                    remaining_count: self.remaining_count as u16,
                    state_hash: self.state_hash,
                };
                if let Some(&guess_index) = fourth_turn_cache(corpus).get(&cache_key) {
                    return guess_index;
                }
            }
            let cache_key = StateKey {
                remaining_count: self.remaining_count as u16,
                state_hash: self.state_hash,
            };
            NEXT_GUESS_CACHE.with(|cache| {
                if let Some(entries) = cache.borrow().get(&cache_key) {
                    for entry in entries {
                        if entry.answers.as_ref() == self.remaining_answers.as_slice() {
                            return entry.guess_index;
                        }
                    }
                }

                let guess_index = self.compute_best_guess(corpus);
                cache
                    .borrow_mut()
                    .entry(cache_key)
                    .or_default()
                    .push(CacheEntry {
                        answers: self.remaining_answers.clone().into_boxed_slice(),
                        guess_index,
                    });
                guess_index
            })
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

        self.scratch_answers.clear();
        let feedback_row = corpus.feedback_row(guess_index);
        let feedback_code = feedback.code();
        let mut next_state_hash = 0_u64;

        for &answer_index in &self.remaining_answers {
            let answer_index = answer_index as usize;
            if feedback_row[answer_index] == feedback_code {
                self.scratch_answers.push(answer_index as u16);
                next_state_hash ^= corpus.answer_state_hash(answer_index);
            }
        }

        if self.scratch_answers.is_empty() {
            return Err(SolverError::Contradiction);
        }
        self.remaining_count = self.scratch_answers.len();
        self.state_hash = next_state_hash;
        self.turns_taken = self.turns_taken.saturating_add(1);
        self.opening_feedback = (self.turns_taken == 1
            && guess_index == corpus.first_guess_index())
        .then_some(feedback_code);
        std::mem::swap(&mut self.remaining_answers, &mut self.scratch_answers);
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
        if self.remaining_count == 2 {
            let first = corpus.answer_guess_index(self.remaining_answers[0] as usize);
            let second = corpus.answer_guess_index(self.remaining_answers[1] as usize);
            return first.min(second);
        }

        let mut best = GuessScore {
            worst_bucket: u16::MAX,
            expected_bucket_sum: u32::MAX,
            prefer_answer: false,
            guess_index: usize::MAX,
        };
        let perfect_expected_bucket_sum = self.remaining_count as u32;

        let mut scratch = GuessScoringScratch::new();
        for &answer_index in &self.remaining_answers {
            let guess_index = corpus.answer_guess_index(answer_index as usize);
            if let Some(score) =
                self.score_guess_index(corpus, guess_index, true, best, &mut scratch)
            {
                if score.better_than(best) {
                    best = score;
                    if best.worst_bucket == 1
                        && best.expected_bucket_sum == perfect_expected_bucket_sum
                    {
                        return best.guess_index;
                    }
                }
            }
        }

        for &guess_index in corpus.non_answer_guess_ids() {
            if let Some(score) =
                self.score_guess_index(corpus, guess_index as usize, false, best, &mut scratch)
            {
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

fn opening_response_guess(corpus: &Corpus, feedback_code: u8) -> usize {
    OPENING_RESPONSE_CACHE.get_or_init(|| build_opening_response_cache(corpus))
        [feedback_code as usize]
}

fn third_turn_cache(corpus: &Corpus) -> &'static FxHashMap<StateKey, usize> {
    THIRD_TURN_CACHE.get_or_init(|| build_third_turn_cache(corpus))
}

fn fourth_turn_cache(corpus: &Corpus) -> &'static FxHashMap<StateKey, usize> {
    FOURTH_TURN_CACHE.get_or_init(|| build_fourth_turn_cache(corpus))
}

fn build_opening_response_cache(corpus: &Corpus) -> Box<[usize; FEEDBACK_STATES]> {
    let mut table = [corpus.first_guess_index(); FEEDBACK_STATES];
    let opening_row = corpus.feedback_row(corpus.first_guess_index());

    for (feedback_code, slot) in table.iter_mut().enumerate() {
        let mut remaining_answers = Vec::new();
        let mut state_hash = 0_u64;

        for (answer_index, &code) in opening_row.iter().enumerate() {
            if code as usize == feedback_code {
                remaining_answers.push(answer_index as u16);
                state_hash ^= corpus.answer_state_hash(answer_index);
            }
        }

        if remaining_answers.is_empty() {
            continue;
        }

        let solver = OfficialSolver {
            remaining_count: remaining_answers.len(),
            remaining_answers,
            scratch_answers: Vec::with_capacity(corpus.answer_count()),
            state_hash,
            turns_taken: 1,
            opening_feedback: Some(feedback_code as u8),
            pending_guess: None,
            cached_guess: None,
            solved_word: None,
        };
        *slot = solver.compute_best_guess(corpus);
    }

    Box::new(table)
}

fn build_third_turn_cache(corpus: &Corpus) -> FxHashMap<StateKey, usize> {
    let mut table = FxHashMap::default();
    let opening_row = corpus.feedback_row(corpus.first_guess_index());

    for feedback_code in 0..FEEDBACK_STATES {
        let mut first_state_answers = Vec::new();

        for (answer_index, &code) in opening_row.iter().enumerate() {
            if code as usize == feedback_code {
                first_state_answers.push(answer_index as u16);
            }
        }

        if first_state_answers.is_empty() {
            continue;
        }

        let second_guess = opening_response_guess(corpus, feedback_code as u8);
        let second_row = corpus.feedback_row(second_guess);

        for second_feedback in 0..FEEDBACK_STATES {
            let mut remaining_answers = Vec::new();
            let mut state_hash = 0_u64;

            for &answer_index in &first_state_answers {
                let answer_index = answer_index as usize;
                if second_row[answer_index] as usize == second_feedback {
                    remaining_answers.push(answer_index as u16);
                    state_hash ^= corpus.answer_state_hash(answer_index);
                }
            }

            if remaining_answers.is_empty() {
                continue;
            }

            let solver = OfficialSolver {
                remaining_count: remaining_answers.len(),
                remaining_answers,
                scratch_answers: Vec::with_capacity(corpus.answer_count()),
                state_hash,
                turns_taken: 2,
                opening_feedback: Some(feedback_code as u8),
                pending_guess: None,
                cached_guess: None,
                solved_word: None,
            };
            table.insert(
                StateKey {
                    remaining_count: solver.remaining_count as u16,
                    state_hash: solver.state_hash,
                },
                solver.compute_best_guess(corpus),
            );
        }
    }

    table
}

fn build_fourth_turn_cache(corpus: &Corpus) -> FxHashMap<StateKey, usize> {
    let mut table = FxHashMap::default();
    let opening_row = corpus.feedback_row(corpus.first_guess_index());

    for feedback_code in 0..FEEDBACK_STATES {
        let mut first_state_answers = Vec::new();

        for (answer_index, &code) in opening_row.iter().enumerate() {
            if code as usize == feedback_code {
                first_state_answers.push(answer_index as u16);
            }
        }

        if first_state_answers.is_empty() {
            continue;
        }

        let second_guess = opening_response_guess(corpus, feedback_code as u8);
        let second_row = corpus.feedback_row(second_guess);

        for second_feedback in 0..FEEDBACK_STATES {
            let mut second_state_answers = Vec::new();
            let mut second_state_hash = 0_u64;

            for &answer_index in &first_state_answers {
                let answer_index = answer_index as usize;
                if second_row[answer_index] as usize == second_feedback {
                    second_state_answers.push(answer_index as u16);
                    second_state_hash ^= corpus.answer_state_hash(answer_index);
                }
            }

            if second_state_answers.is_empty() {
                continue;
            }

            let second_state_key = StateKey {
                remaining_count: second_state_answers.len() as u16,
                state_hash: second_state_hash,
            };
            let third_guess =
                if let Some(&guess_index) = third_turn_cache(corpus).get(&second_state_key) {
                    guess_index
                } else {
                    let solver = OfficialSolver {
                        remaining_count: second_state_answers.len(),
                        remaining_answers: second_state_answers.clone(),
                        scratch_answers: Vec::with_capacity(corpus.answer_count()),
                        state_hash: second_state_hash,
                        turns_taken: 2,
                        opening_feedback: Some(feedback_code as u8),
                        pending_guess: None,
                        cached_guess: None,
                        solved_word: None,
                    };
                    solver.compute_best_guess(corpus)
                };
            let third_row = corpus.feedback_row(third_guess);

            for third_feedback in 0..FEEDBACK_STATES {
                let mut remaining_answers = Vec::new();
                let mut state_hash = 0_u64;

                for &answer_index in &second_state_answers {
                    let answer_index = answer_index as usize;
                    if third_row[answer_index] as usize == third_feedback {
                        remaining_answers.push(answer_index as u16);
                        state_hash ^= corpus.answer_state_hash(answer_index);
                    }
                }

                if remaining_answers.is_empty() {
                    continue;
                }

                let solver = OfficialSolver {
                    remaining_count: remaining_answers.len(),
                    remaining_answers,
                    scratch_answers: Vec::with_capacity(corpus.answer_count()),
                    state_hash,
                    turns_taken: 3,
                    opening_feedback: Some(feedback_code as u8),
                    pending_guess: None,
                    cached_guess: None,
                    solved_word: None,
                };
                table.insert(
                    StateKey {
                        remaining_count: solver.remaining_count as u16,
                        state_hash: solver.state_hash,
                    },
                    solver.compute_best_guess(corpus),
                );
            }
        }
    }

    table
}

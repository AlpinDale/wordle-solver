use crate::corpus::Corpus;
use crate::{Feedback, SolverError, Word, FEEDBACK_STATES};

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

        Ok(Self {
            remaining,
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
            .iter_remaining_answers()
            .map(|answer_index| corpus.answer_word(answer_index))
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
        let guess_index = self
            .cached_guess
            .unwrap_or_else(|| self.compute_best_guess(corpus));
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

        let mut next_remaining = self.remaining.clone();
        let mut next_count = 0_usize;

        for answer_index in self.iter_remaining_answers() {
            let matches = corpus.feedback(guess_index, answer_index) == feedback;
            set_bit(&mut next_remaining, answer_index, matches);
            if matches {
                next_count += 1;
            }
        }

        if next_count == 0 {
            return Err(SolverError::Contradiction);
        }

        self.remaining = next_remaining;
        self.remaining_count = next_count;
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
            let Some(answer_index) = self.iter_remaining_answers().next() else {
                debug_assert!(
                    false,
                    "single-answer state should have one remaining answer"
                );
                return corpus.first_guess_index();
            };
            return corpus.answer_guess_index(answer_index);
        }

        let mut best = GuessScore {
            worst_bucket: u16::MAX,
            expected_bucket_sum: u32::MAX,
            prefer_answer: false,
            guess_index: usize::MAX,
        };

        let mut histogram = [0_u16; FEEDBACK_STATES];
        for guess_index in 0..corpus.guess_count() {
            histogram.fill(0);

            for answer_index in self.iter_remaining_answers() {
                let feedback = corpus.feedback(guess_index, answer_index);
                histogram[feedback.code() as usize] += 1;
            }

            let mut worst_bucket = 0_u16;
            let mut expected_bucket_sum = 0_u32;
            for &bucket_size in &histogram {
                worst_bucket = worst_bucket.max(bucket_size);
                expected_bucket_sum += u32::from(bucket_size) * u32::from(bucket_size);
            }

            let score = GuessScore {
                worst_bucket,
                expected_bucket_sum,
                prefer_answer: self.guess_is_remaining_answer(corpus, guess_index),
                guess_index,
            };

            if score.better_than(best) {
                best = score;
            }
        }

        best.guess_index
    }

    fn guess_is_remaining_answer(&self, corpus: &Corpus, guess_index: usize) -> bool {
        if !corpus.is_answer_guess(guess_index) {
            return false;
        }
        corpus
            .find_answer(corpus.guess_word(guess_index))
            .is_some_and(|answer_index| get_bit(&self.remaining, answer_index))
    }

    fn iter_remaining_answers(&self) -> impl Iterator<Item = usize> + '_ {
        self.remaining
            .iter()
            .enumerate()
            .flat_map(|(block_index, &block)| BitBlockIterator {
                block,
                base_index: block_index * 64,
            })
    }

    fn issue_guess_index(&mut self, guess_index: usize, corpus: &Corpus) -> Word {
        self.pending_guess = Some(guess_index);
        corpus.guess_word(guess_index)
    }
}

struct BitBlockIterator {
    block: u64,
    base_index: usize,
}

impl Iterator for BitBlockIterator {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.block == 0 {
            return None;
        }
        let offset = self.block.trailing_zeros() as usize;
        self.block &= self.block - 1;
        Some(self.base_index + offset)
    }
}

fn get_bit(words: &[u64], index: usize) -> bool {
    (words[index / 64] >> (index % 64)) & 1 == 1
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

use std::sync::OnceLock;

use crate::asset::LoadedBundle;
use crate::{Feedback, SolverError, Word};

static BUNDLE_BYTES: &[u8] = include_bytes!("../../../data/official/assets/official.bundle");
static CORPUS: OnceLock<Result<Corpus, SolverError>> = OnceLock::new();

#[derive(Debug)]
pub(crate) struct Corpus {
    corpus_hash: u64,
    guesses: Box<[Word]>,
    answer_ids: Box<[u16]>,
    answer_positions: Box<[u16]>,
    feedback_matrix: Box<[u8]>,
    first_guess_index: usize,
}

impl Corpus {
    pub fn load() -> Result<&'static Self, SolverError> {
        CORPUS
            .get_or_init(|| Self::from_bundle(LoadedBundle::parse(BUNDLE_BYTES)?))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn from_bundle(bundle: LoadedBundle) -> Result<Self, SolverError> {
        let mut answer_positions = vec![u16::MAX; bundle.guesses.len()].into_boxed_slice();
        for (answer_index, &guess_index) in bundle.answer_ids.iter().enumerate() {
            answer_positions[guess_index as usize] =
                u16::try_from(answer_index).map_err(|_| SolverError::AssetCorrupt)?;
        }

        Ok(Self {
            corpus_hash: bundle.corpus_hash,
            guesses: bundle.guesses,
            answer_ids: bundle.answer_ids,
            answer_positions,
            feedback_matrix: bundle.feedback_matrix,
            first_guess_index: bundle.first_guess_index,
        })
    }

    pub fn corpus_hash(&self) -> u64 {
        self.corpus_hash
    }

    pub fn guess_count(&self) -> usize {
        self.guesses.len()
    }

    pub fn answer_count(&self) -> usize {
        self.answer_ids.len()
    }

    pub fn first_guess_index(&self) -> usize {
        self.first_guess_index
    }

    pub fn guess_word(&self, guess_index: usize) -> Word {
        self.guesses[guess_index]
    }

    pub fn answer_word(&self, answer_index: usize) -> Word {
        self.guesses[self.answer_ids[answer_index] as usize]
    }

    pub fn answer_guess_index(&self, answer_index: usize) -> usize {
        self.answer_ids[answer_index] as usize
    }

    pub fn find_guess(&self, word: Word) -> Option<usize> {
        self.guesses.binary_search(&word).ok()
    }

    pub fn find_answer(&self, word: Word) -> Option<usize> {
        self.find_guess(word).and_then(|guess_index| {
            let answer_index = self.answer_positions[guess_index];
            (answer_index != u16::MAX).then_some(answer_index as usize)
        })
    }

    pub fn feedback(&self, guess_index: usize, answer_index: usize) -> Feedback {
        let matrix_index = guess_index * self.answer_count() + answer_index;
        Feedback::from_code(self.feedback_matrix[matrix_index])
    }

    pub fn is_answer_guess(&self, guess_index: usize) -> bool {
        self.answer_positions[guess_index] != u16::MAX
    }
}

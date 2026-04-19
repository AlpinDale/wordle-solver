mod asset;
mod corpus;
mod solver;
mod types;

use std::fmt;
use std::io;

pub use asset::{BundleData, LoadedBundle, BUNDLE_VERSION};
pub use solver::{OfficialSolver, SolveStep, SolveTrace, SolverStatus};
pub use types::{score_guess, Feedback, Word, FEEDBACK_STATES, WORD_LEN};

#[derive(Debug, Clone)]
pub enum SolverError {
    InvalidWord,
    InvalidFeedback,
    UnknownAnswer,
    UnknownGuess,
    Contradiction,
    AlreadySolved,
    GuessNotIssued,
    AssetCorrupt,
    Io(io::ErrorKind),
}

impl SolverError {
    pub(crate) fn io(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

impl From<io::Error> for SolverError {
    fn from(error: io::Error) -> Self {
        Self::io(error)
    }
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWord => f.write_str("invalid word"),
            Self::InvalidFeedback => f.write_str("invalid feedback"),
            Self::UnknownAnswer => f.write_str("unknown answer"),
            Self::UnknownGuess => f.write_str("unknown guess"),
            Self::Contradiction => f.write_str("feedback contradicts previous constraints"),
            Self::AlreadySolved => f.write_str("solver is already solved"),
            Self::GuessNotIssued => f.write_str("no pending solver guess"),
            Self::AssetCorrupt => f.write_str("bundled strategy asset is corrupt"),
            Self::Io(kind) => write!(f, "i/o error: {kind:?}"),
        }
    }
}

impl std::error::Error for SolverError {}

pub fn bundled_corpus_hash() -> Result<u64, SolverError> {
    Ok(corpus::Corpus::load()?.corpus_hash())
}

pub fn bundled_guess_count() -> Result<usize, SolverError> {
    Ok(corpus::Corpus::load()?.guess_count())
}

pub fn bundled_answer_count() -> Result<usize, SolverError> {
    Ok(corpus::Corpus::load()?.answer_count())
}

pub fn bundled_opening_guess() -> Result<Word, SolverError> {
    let corpus = corpus::Corpus::load()?;
    Ok(corpus.guess_word(corpus.first_guess_index()))
}

pub fn bundled_answers() -> Result<Vec<Word>, SolverError> {
    let corpus = corpus::Corpus::load()?;
    Ok((0..corpus.answer_count())
        .map(|answer_index| corpus.answer_word(answer_index))
        .collect())
}

pub fn bundled_guesses() -> Result<Vec<Word>, SolverError> {
    let corpus = corpus::Corpus::load()?;
    Ok((0..corpus.guess_count())
        .map(|guess_index| corpus.guess_word(guess_index))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_letters_are_scored_correctly() {
        let guess = Word::parse("allee").expect("test word should parse");
        let answer = Word::parse("apple").expect("test word should parse");
        assert_eq!(score_guess(guess, answer).as_string(), "gybbg");
    }

    #[test]
    fn bundled_asset_is_loadable() {
        let solver = OfficialSolver::try_new().expect("bundle should load");
        assert!(solver.remaining_answers() > 2000);
    }

    #[test]
    fn feedback_roundtrip_accepts_letters() {
        let feedback = Feedback::parse("bgybg").expect("test feedback should parse");
        assert_eq!(feedback.as_string(), "bgybg");
    }

    #[test]
    fn bundled_metadata_is_exposed() {
        assert_eq!(
            bundled_opening_guess()
                .expect("opening guess should load")
                .to_string(),
            "trace"
        );
        assert_eq!(
            bundled_answer_count().expect("answer count should load"),
            2315
        );
        assert!(
            bundled_guess_count().expect("guess count should load")
                > bundled_answer_count().expect("answer count should load")
        );
    }
}

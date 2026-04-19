use std::fmt;
use std::str::FromStr;

use crate::SolverError;

pub const WORD_LEN: usize = 5;
pub const FEEDBACK_STATES: usize = 243;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Word(u32);

impl Word {
    pub const fn from_packed(packed: u32) -> Self {
        Self(packed)
    }

    pub const fn packed(self) -> u32 {
        self.0
    }

    pub fn parse(input: &str) -> Result<Self, SolverError> {
        Self::from_str(input)
    }

    pub fn letters(self) -> [u8; WORD_LEN] {
        let mut packed = self.0;
        let mut letters = [b'a'; WORD_LEN];
        let mut index = 0;
        while index < WORD_LEN {
            letters[index] = b'a' + (packed & 0x1f) as u8;
            packed >>= 5;
            index += 1;
        }
        letters
    }

    pub fn as_string(self) -> String {
        let letters = self.letters();
        letters.into_iter().map(char::from).collect()
    }
}

impl FromStr for Word {
    type Err = SolverError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if input.len() != WORD_LEN {
            return Err(SolverError::InvalidWord);
        }

        let mut packed = 0_u32;
        for (shift, byte) in input.bytes().enumerate() {
            if !byte.is_ascii_lowercase() {
                return Err(SolverError::InvalidWord);
            }
            packed |= u32::from(byte - b'a') << (shift * 5);
        }

        Ok(Self(packed))
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Feedback(u8);

impl Feedback {
    pub const MISS: u8 = 0;
    pub const PRESENT: u8 = 1;
    pub const EXACT: u8 = 2;
    pub const SOLVED: Self = Self(242);

    pub const fn from_code(code: u8) -> Self {
        Self(code)
    }

    pub const fn code(self) -> u8 {
        self.0
    }

    pub fn parse(input: &str) -> Result<Self, SolverError> {
        if input.len() != WORD_LEN {
            return Err(SolverError::InvalidFeedback);
        }

        let mut code = 0_u8;
        let mut place = 1_u8;
        for byte in input.bytes() {
            let digit = match byte {
                b'b' | b'B' | b'0' => Self::MISS,
                b'y' | b'Y' | b'1' => Self::PRESENT,
                b'g' | b'G' | b'2' => Self::EXACT,
                _ => return Err(SolverError::InvalidFeedback),
            };
            code = code.saturating_add(digit.saturating_mul(place));
            place = place.saturating_mul(3);
        }

        Ok(Self(code))
    }

    pub fn cells(self) -> [u8; WORD_LEN] {
        let mut cells = [0_u8; WORD_LEN];
        let mut code = self.0;
        let mut index = 0;
        while index < WORD_LEN {
            cells[index] = code % 3;
            code /= 3;
            index += 1;
        }
        cells
    }

    pub fn as_string(self) -> String {
        self.cells()
            .into_iter()
            .map(|cell| match cell {
                Self::MISS => 'b',
                Self::PRESENT => 'y',
                Self::EXACT => 'g',
                _ => unreachable!(),
            })
            .collect()
    }

    pub const fn is_solved(self) -> bool {
        self.0 == Self::SOLVED.0
    }
}

impl fmt::Display for Feedback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_string())
    }
}

pub fn score_guess(guess: Word, answer: Word) -> Feedback {
    let guess_letters = guess.letters();
    let answer_letters = answer.letters();

    let mut counts = [0_u8; 26];
    let mut cells = [Feedback::MISS; WORD_LEN];

    let mut index = 0;
    while index < WORD_LEN {
        if guess_letters[index] == answer_letters[index] {
            cells[index] = Feedback::EXACT;
        } else {
            counts[(answer_letters[index] - b'a') as usize] += 1;
        }
        index += 1;
    }

    index = 0;
    while index < WORD_LEN {
        if cells[index] == Feedback::EXACT {
            index += 1;
            continue;
        }

        let letter_index = (guess_letters[index] - b'a') as usize;
        if counts[letter_index] > 0 {
            cells[index] = Feedback::PRESENT;
            counts[letter_index] -= 1;
        }
        index += 1;
    }

    let mut code = 0_u8;
    let mut place = 1_u8;
    index = 0;
    while index < WORD_LEN {
        code += cells[index] * place;
        place *= 3;
        index += 1;
    }

    Feedback::from_code(code)
}

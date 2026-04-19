use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::{SolverError, Word};

const MAGIC: &[u8; 4] = b"WDL1";
pub const BUNDLE_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct BundleData {
    pub corpus_hash: u64,
    pub first_guess_index: u32,
    pub guesses: Vec<Word>,
    pub answer_ids: Vec<u16>,
    pub feedback_matrix: Vec<u8>,
}

impl BundleData {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            32 + self.guesses.len() * 4 + self.answer_ids.len() * 2 + self.feedback_matrix.len(),
        );

        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&BUNDLE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.corpus_hash.to_le_bytes());
        bytes.extend_from_slice(&(self.guesses.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.answer_ids.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.first_guess_index.to_le_bytes());

        for word in &self.guesses {
            bytes.extend_from_slice(&word.packed().to_le_bytes());
        }
        for answer_id in &self.answer_ids {
            bytes.extend_from_slice(&answer_id.to_le_bytes());
        }
        bytes.extend_from_slice(&self.feedback_matrix);

        bytes
    }

    pub fn write_to_path(&self, path: &Path) -> Result<(), SolverError> {
        fs::write(path, self.encode()).map_err(Into::into)
    }
}

#[derive(Clone, Debug)]
pub struct LoadedBundle {
    pub corpus_hash: u64,
    pub first_guess_index: usize,
    pub guesses: Box<[Word]>,
    pub answer_ids: Box<[u16]>,
    pub feedback_matrix: Box<[u8]>,
}

impl LoadedBundle {
    pub fn parse(bytes: &[u8]) -> Result<Self, SolverError> {
        if bytes.len() < 28 || &bytes[0..4] != MAGIC {
            return Err(SolverError::AssetCorrupt);
        }

        let version = read_u32(bytes, 4)?;
        if version != BUNDLE_VERSION {
            return Err(SolverError::AssetCorrupt);
        }

        let corpus_hash = read_u64(bytes, 8)?;
        let guess_count = read_u32(bytes, 16)? as usize;
        let answer_count = read_u32(bytes, 20)? as usize;
        let first_guess_index = read_u32(bytes, 24)? as usize;

        let guess_bytes = guess_count
            .checked_mul(4)
            .ok_or(SolverError::AssetCorrupt)?;
        let answer_bytes = answer_count
            .checked_mul(2)
            .ok_or(SolverError::AssetCorrupt)?;
        let matrix_bytes = guess_count
            .checked_mul(answer_count)
            .ok_or(SolverError::AssetCorrupt)?;
        let expected_len = 28_usize
            .checked_add(guess_bytes)
            .and_then(|value| value.checked_add(answer_bytes))
            .and_then(|value| value.checked_add(matrix_bytes))
            .ok_or(SolverError::AssetCorrupt)?;

        if bytes.len() != expected_len || first_guess_index >= guess_count {
            return Err(SolverError::AssetCorrupt);
        }

        let mut offset = 28;

        let guesses = bytes[offset..offset + guess_bytes]
            .chunks_exact(4)
            .map(read_word)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        offset += guess_bytes;

        let answer_ids = bytes[offset..offset + answer_bytes]
            .chunks_exact(2)
            .map(read_u16)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        offset += answer_bytes;

        let guess_len_u16 = u16::try_from(guess_count).map_err(|_| SolverError::AssetCorrupt)?;
        let mut seen = BTreeSet::new();
        for &answer_id in &answer_ids {
            if answer_id >= guess_len_u16 || !seen.insert(answer_id) {
                return Err(SolverError::AssetCorrupt);
            }
        }

        let feedback_matrix = bytes[offset..].to_vec().into_boxed_slice();

        Ok(Self {
            corpus_hash,
            first_guess_index,
            guesses,
            answer_ids,
            feedback_matrix,
        })
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SolverError> {
    let end = offset.checked_add(4).ok_or(SolverError::AssetCorrupt)?;
    let slice = bytes.get(offset..end).ok_or(SolverError::AssetCorrupt)?;
    Ok(read_u32_exact(slice))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, SolverError> {
    let end = offset.checked_add(8).ok_or(SolverError::AssetCorrupt)?;
    let slice = bytes.get(offset..end).ok_or(SolverError::AssetCorrupt)?;
    Ok(read_u64_exact(slice))
}

fn read_word(chunk: &[u8]) -> Word {
    Word::from_packed(read_u32_exact(chunk))
}

fn read_u16(chunk: &[u8]) -> u16 {
    let mut bytes = [0_u8; 2];
    bytes.copy_from_slice(chunk);
    u16::from_le_bytes(bytes)
}

fn read_u32_exact(chunk: &[u8]) -> u32 {
    let mut bytes = [0_u8; 4];
    bytes.copy_from_slice(chunk);
    u32::from_le_bytes(bytes)
}

fn read_u64_exact(chunk: &[u8]) -> u64 {
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(chunk);
    u64::from_le_bytes(bytes)
}

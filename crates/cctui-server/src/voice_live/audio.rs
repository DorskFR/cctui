//! Splits an oversized 16 kHz mono PCM16 utterance into chunks the speech
//! service takes one at a time, cutting at silences where it can.

use bytes::Bytes;

use super::text::UTTERANCE_SAMPLE_RATE;

const BYTES_PER_SECOND: usize = 2 * UTTERANCE_SAMPLE_RATE as usize;
/// Longest chunk handed to the speech service; the browser caps its parts
/// at the same length.
pub const MAX_CHUNK_BYTES: usize = BYTES_PER_SECOND * 30;
/// A cut is not looked for before this much of a chunk.
const MIN_CHUNK_BYTES: usize = BYTES_PER_SECOND * 10;
/// 20 ms of samples, the grain silence is measured at.
const BLOCK_BYTES: usize = BYTES_PER_SECOND / 50;
/// Mean sample magnitude under which a block counts as silence (about
/// -44 dBFS).
const SILENCE_LEVEL: u32 = 200;

/// `pcm` in sample-aligned chunks of at most `cap` bytes that concatenate
/// back to it. Each cut lands at the quietest 20 ms block in the last two
/// thirds of the chunk when one is silent, else at the cap.
pub fn split_utterance(pcm: Bytes, cap: usize) -> Vec<Bytes> {
    let cap = (cap.max(2 * BLOCK_BYTES)) & !1;
    let min = (MIN_CHUNK_BYTES.min(cap / 3)) & !1;
    let mut out = Vec::new();
    let mut at = 0;
    while pcm.len() - at > cap {
        let cut = at + silence_cut(&pcm[at + min..at + cap]).map_or(cap, |c| min + c);
        out.push(pcm.slice(at..cut));
        at = cut;
    }
    if at < pcm.len() {
        out.push(pcm.slice(at..));
    }
    out
}

/// The end of the quietest silent block in `window`, as an offset into it.
fn silence_cut(window: &[u8]) -> Option<usize> {
    let mut best: Option<(u32, usize)> = None;
    for (i, block) in window.chunks_exact(BLOCK_BYTES).enumerate() {
        let level = block_level(block);
        if level < SILENCE_LEVEL && best.is_none_or(|(l, _)| level <= l) {
            best = Some((level, (i + 1) * BLOCK_BYTES));
        }
    }
    best.map(|(_, end)| end)
}

fn block_level(block: &[u8]) -> u32 {
    let samples = block.chunks_exact(2);
    let n = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    let sum: u32 =
        samples.map(|s| u32::from(i16::from_le_bytes([s[0], s[1]]).unsigned_abs())).sum();
    sum / n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: usize) -> Vec<u8> {
        [0x00, 0x40].repeat(seconds * BYTES_PER_SECOND / 2)
    }

    fn silence(seconds: usize) -> Vec<u8> {
        vec![0; seconds * BYTES_PER_SECOND]
    }

    fn rejoined(chunks: &[Bytes]) -> Vec<u8> {
        chunks.iter().flat_map(|c| c.iter().copied()).collect()
    }

    #[test]
    fn a_short_or_empty_utterance_is_one_chunk_or_none() {
        assert!(split_utterance(Bytes::new(), MAX_CHUNK_BYTES).is_empty());
        let tiny = Bytes::from_static(&[0, 0]);
        assert_eq!(split_utterance(tiny.clone(), MAX_CHUNK_BYTES), vec![tiny]);
        let fits = Bytes::from(tone(30));
        assert_eq!(split_utterance(fits.clone(), MAX_CHUNK_BYTES), vec![fits]);
    }

    #[test]
    fn a_continuous_tone_is_cut_at_the_cap() {
        let pcm = Bytes::from(tone(5 * 60));
        let chunks = split_utterance(pcm.clone(), MAX_CHUNK_BYTES);
        assert_eq!(chunks.len(), 10);
        assert!(chunks.iter().all(|c| c.len() == MAX_CHUNK_BYTES), "{:?}", lens(&chunks));
        assert_eq!(rejoined(&chunks), pcm);
    }

    #[test]
    fn a_cut_lands_in_a_silence_when_there_is_one() {
        let mut pcm = tone(20);
        pcm.extend(silence(1));
        pcm.extend(tone(20));
        let pcm = Bytes::from(pcm);
        let chunks = split_utterance(pcm.clone(), MAX_CHUNK_BYTES);
        assert_eq!(chunks.len(), 2, "{:?}", lens(&chunks));
        let first = chunks[0].len();
        assert!((20 * BYTES_PER_SECOND..=21 * BYTES_PER_SECOND).contains(&first), "{first}");
        assert_eq!(rejoined(&chunks), pcm);
    }

    #[test]
    fn a_silence_before_the_minimum_is_not_a_cut() {
        let mut pcm = tone(2);
        pcm.extend(silence(1));
        pcm.extend(tone(40));
        let pcm = Bytes::from(pcm);
        let chunks = split_utterance(pcm.clone(), MAX_CHUNK_BYTES);
        assert!(chunks[0].len() >= MIN_CHUNK_BYTES, "{:?}", lens(&chunks));
        assert_eq!(rejoined(&chunks), pcm);
    }

    #[test]
    fn chunks_are_sample_aligned_for_any_cap() {
        let pcm = Bytes::from(tone(7));
        for cap in [BLOCK_BYTES * 2 + 1, 12_345, 100_001, MAX_CHUNK_BYTES] {
            let chunks = split_utterance(pcm.clone(), cap);
            assert!(chunks.iter().all(|c| c.len() % 2 == 0 && c.len() <= (cap | 1)), "{cap}");
            assert_eq!(rejoined(&chunks), pcm, "{cap}");
        }
    }

    #[test]
    fn a_silent_block_is_below_the_level_and_a_tone_above() {
        assert_eq!(block_level(&[0; BLOCK_BYTES]), 0);
        assert!(block_level(&tone(1)[..BLOCK_BYTES]) > SILENCE_LEVEL);
    }

    fn lens(chunks: &[Bytes]) -> Vec<usize> {
        chunks.iter().map(Bytes::len).collect()
    }
}

//! Cuts ffmpeg's AVI output back into encoded frames.
//!
//! Raw Annex B has no end-of-frame marker: a frame only ends where a later one
//! starts. UScreen sends frames only when the screen changes, so "later" can be
//! hundreds of milliseconds away. An AVI chunk states its size up front and
//! carries the encoder's packet untouched, so a frame can leave the moment it
//! is complete.

use anyhow::{bail, Result};

const MOVI: &[u8] = b"movi";
const VIDEO_CHUNK: &[u8] = b"00dc";
const CHUNK_HEADER_BYTES: usize = 8;
/// ffmpeg's AVI header is a few kilobytes. Anything past this is not AVI.
const MAX_HEADER_BYTES: usize = 1 << 20;
/// Far above any real frame. A larger size means the stream lost sync.
const MAX_CHUNK_BYTES: usize = 256 << 20;

#[derive(Default)]
pub struct AviDemuxer {
    buffer: Vec<u8>,
    in_movi: bool,
}

impl AviDemuxer {
    /// Add bytes from the encoder and return every frame they complete.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.buffer.extend_from_slice(data);
        if !self.in_movi && !self.skip_header()? {
            return Ok(Vec::new());
        }

        let mut frames = Vec::new();
        let mut pos = 0;
        while let Some(header) = self.buffer.get(pos..pos + CHUNK_HEADER_BYTES) {
            let (tag, size) = header.split_at(4);
            let size = u32::from_le_bytes([size[0], size[1], size[2], size[3]]) as usize;
            if size > MAX_CHUNK_BYTES {
                bail!("AVI chunk of {} bytes: the encoder stream lost sync", size);
            }

            let start = pos + CHUNK_HEADER_BYTES;
            // AVI pads every chunk to an even length.
            let next = start + size + (size & 1);
            if next > self.buffer.len() {
                break;
            }
            // ffmpeg writes empty chunks for skipped timestamps.
            if tag == VIDEO_CHUNK && size > 0 {
                frames.push(self.buffer[start..start + size].to_vec());
            }
            pos = next;
        }
        self.buffer.drain(..pos);
        Ok(frames)
    }

    /// Drop everything up to the first chunk. Searches for the `movi` list
    /// rather than walking the header: ffmpeg cannot seek back on a pipe to
    /// fill in list sizes, so they are not reliable here.
    fn skip_header(&mut self) -> Result<bool> {
        match self.buffer.windows(MOVI.len()).position(|w| w == MOVI) {
            Some(at) => {
                self.buffer.drain(..at + MOVI.len());
                self.in_movi = true;
                Ok(true)
            }
            None if self.buffer.len() > MAX_HEADER_BYTES => {
                bail!("no AVI frame data in the first {} bytes", MAX_HEADER_BYTES)
            }
            None => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(tag: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut data = tag.to_vec();
        data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            data.push(0);
        }
        data
    }

    fn header() -> Vec<u8> {
        let mut data = b"RIFF\0\0\0\0AVI LIST\xc0\0\0\0hdrl".to_vec();
        data.extend_from_slice(&[0; 192]);
        data.extend_from_slice(b"LIST\0\0\0\0movi");
        data
    }

    #[test]
    fn returns_a_frame_as_soon_as_its_chunk_is_complete() {
        let mut avi = AviDemuxer::default();
        assert!(avi.push(&header()).unwrap().is_empty());
        assert_eq!(avi.push(&chunk(b"00dc", b"frame")).unwrap(), [b"frame".to_vec()]);
    }

    #[test]
    fn reassembles_a_frame_split_across_reads() {
        let mut avi = AviDemuxer::default();
        let mut data = header();
        data.extend(chunk(b"00dc", b"frame"));
        let (first, rest) = data.split_at(data.len() - 3);
        assert!(avi.push(first).unwrap().is_empty());
        assert_eq!(avi.push(rest).unwrap(), [b"frame".to_vec()]);
    }

    #[test]
    fn finds_the_frame_data_when_the_marker_is_split_across_reads() {
        let mut avi = AviDemuxer::default();
        let mut data = header();
        data.extend(chunk(b"00dc", b"frame"));
        let split = data.windows(4).position(|w| w == MOVI).unwrap() + 2;
        assert!(avi.push(&data[..split]).unwrap().is_empty());
        assert_eq!(avi.push(&data[split..]).unwrap(), [b"frame".to_vec()]);
    }

    #[test]
    fn splits_several_frames_in_one_read_and_honours_padding() {
        let mut avi = AviDemuxer::default();
        let mut data = header();
        data.extend(chunk(b"00dc", b"odd"));
        data.extend(chunk(b"00dc", b"even"));
        assert_eq!(avi.push(&data).unwrap(), [b"odd".to_vec(), b"even".to_vec()]);
    }

    #[test]
    fn skips_empty_and_unrelated_chunks() {
        let mut avi = AviDemuxer::default();
        let mut data = header();
        data.extend(chunk(b"00dc", b""));
        data.extend(chunk(b"JUNK", b"padding"));
        data.extend(chunk(b"00dc", b"frame"));
        assert_eq!(avi.push(&data).unwrap(), [b"frame".to_vec()]);
    }

    #[test]
    fn rejects_a_chunk_size_that_means_lost_sync() {
        let mut avi = AviDemuxer::default();
        let mut data = header();
        data.extend_from_slice(b"00dc\xff\xff\xff\xff");
        assert!(avi.push(&data).is_err());
    }

    #[test]
    fn rejects_a_stream_that_is_not_avi() {
        let mut avi = AviDemuxer::default();
        assert!(avi.push(&vec![0; MAX_HEADER_BYTES + 1]).is_err());
    }
}

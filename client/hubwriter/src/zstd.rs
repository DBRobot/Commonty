use std::io::{self, BufRead, Read};

use ruzstd::decoding::errors::{FrameDecoderError, ReadFrameHeaderError};
use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

/// Every frame of a zstd stream, skippable ones skipped; ruzstd's own
/// StreamingDecoder stops after the first.
pub struct ZstdStream<R> {
    src: R,
    dec: FrameDecoder,
    in_frame: bool,
}

impl<R: BufRead> ZstdStream<R> {
    pub fn new(src: R) -> Self {
        ZstdStream {
            src,
            dec: FrameDecoder::new(),
            in_frame: false,
        }
    }

    pub fn get_ref(&self) -> &R {
        &self.src
    }

    fn next_frame(&mut self) -> io::Result<bool> {
        loop {
            if self.src.fill_buf()?.is_empty() {
                return Ok(false);
            }
            match self.dec.reset(&mut self.src) {
                Ok(()) => return Ok(true),
                Err(FrameDecoderError::ReadFrameHeaderError(ReadFrameHeaderError::SkipFrame {
                    length,
                    ..
                })) => {
                    let skipped =
                        io::copy(&mut (&mut self.src).take(length as u64), &mut io::sink())?;
                    if skipped != length as u64 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "truncated skippable frame",
                        ));
                    }
                }
                Err(e) => return Err(io::Error::other(e)),
            }
        }
    }
}

impl<R: BufRead> Read for ZstdStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if !self.in_frame {
                if !self.next_frame()? {
                    return Ok(0);
                }
                self.in_frame = true;
            }
            while self.dec.can_collect() < buf.len() && !self.dec.is_finished() {
                let want = buf.len() - self.dec.can_collect();
                self.dec
                    .decode_blocks(&mut self.src, BlockDecodingStrategy::UptoBytes(want))
                    .map_err(io::Error::other)?;
            }
            let n = self.dec.read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            if self.dec.is_finished() {
                if let (Some(want), Some(got)) = (
                    self.dec.get_checksum_from_data(),
                    self.dec.get_calculated_checksum(),
                ) && want != got
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "zstd frame checksum mismatch",
                    ));
                }
                self.in_frame = false;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "zstd frame ends early",
                ));
            }
        }
    }
}

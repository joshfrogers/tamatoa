// Copyright 2021 Colin Finck <colin@reactos.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::io;
use std::io::{Read, Seek, SeekFrom};

/// `SectorReader` encapsulates any reader and only performs read and seek operations on it
/// on boundaries of the given sector size.
///
/// This can be very useful for readers that only accept sector-sized reads (like reading
/// from a raw partition on Windows).
/// The sector size must be a power of two.
///
/// This reader does not keep any buffer of accumulated data; it only keeps a scratch
/// allocation that is reused between reads.
/// You are advised to encapsulate `SectorReader` in a buffered reader, as unbuffered reads of
/// just a few bytes here and there are highly inefficient.
///
/// Callers must `seek` before each `read` (as the `ntfs` crate does); consecutive reads
/// without an intervening seek are not supported, since the raw Windows volume handle has no
/// notion of byte-granular positions.
pub struct SectorReader<R>
where
    R: Read + Seek,
{
    /// The inner reader stream.
    inner: R,
    /// The sector size set at creation.
    sector_size: usize,
    /// The current stream position as requested by the caller through `read` or `seek`.
    /// The implementation will internally make sure to only read/seek on sector boundaries.
    stream_position: u64,
    /// This buffer is only part of the struct as a small performance optimization (keeping it allocated between reads).
    temp_buf: Vec<u8>,
}

impl<R> SectorReader<R>
where
    R: Read + Seek,
{
    pub fn new(inner: R, sector_size: usize) -> io::Result<Self> {
        if sector_size == 0 || !sector_size.is_power_of_two() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sector_size must be a nonzero power of two",
            ));
        }

        Ok(Self {
            inner,
            sector_size,
            stream_position: 0,
            temp_buf: Vec::new(),
        })
    }

    fn align_down_to_sector_size(&self, n: u64) -> u64 {
        n / self.sector_size as u64 * self.sector_size as u64
    }

    fn align_up_to_sector_size(&self, n: u64) -> io::Result<u64> {
        self.align_down_to_sector_size(n)
            .checked_add(self.sector_size as u64)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "sector alignment would overflow",
                )
            })
    }
}

impl<R> Read for SectorReader<R>
where
    R: Read + Seek,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }

        // We can only read from a sector boundary, and `self.stream_position` specifies the position where the
        // caller thinks we are.
        // Align down to a sector boundary to determine the position where we really are (see our `seek` implementation).
        let aligned_position = self.align_down_to_sector_size(self.stream_position);

        // We have to read more bytes now to make up for the alignment difference.
        // We can also only read in multiples of the sector size, so align up to the next sector boundary.
        let start = (self.stream_position - aligned_position) as usize;
        let end = start + buf.len();
        let aligned_bytes_to_read = self.align_up_to_sector_size(end as u64)? as usize;

        // Perform the sector-sized read. Unlike read_exact, hitting the end of the volume is
        // reported as a short read / clean EOF, not an error - a raw volume can be shorter than
        // the last sector-aligned chunk implies, and crafted filesystem metadata can point past
        // the real end of the volume.
        self.temp_buf.resize(aligned_bytes_to_read, 0);
        let mut filled = 0usize;
        while filled < aligned_bytes_to_read {
            match self
                .inner
                .read(&mut self.temp_buf[filled..aligned_bytes_to_read])
            {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }

        if filled <= start {
            // Nothing readable left at the requested position: EOF.
            return Ok(0);
        }

        // Copy the actually requested bytes into the given buffer, shortening at EOF.
        let end = end.min(filled);
        let copied = end - start;
        buf[..copied].copy_from_slice(&self.temp_buf[start..end]);

        self.stream_position += copied as u64;
        Ok(copied)
    }
}

impl<R> Seek for SectorReader<R>
where
    R: Read + Seek,
{
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(_n) => {
                // This is unsupported, because it's not safely possible under Windows.
                // We cannot seek to the end to determine the raw partition size.
                // Which makes it impossible to set `self.stream_position`.
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "SeekFrom::End is unsupported for SectorReader",
                ));
            }
            SeekFrom::Current(n) => {
                if n >= 0 {
                    self.stream_position.checked_add(n as u64)
                } else {
                    self.stream_position.checked_sub(n.wrapping_neg() as u64)
                }
            }
        };

        match new_pos {
            Some(n) => {
                // We can only seek on sector boundaries, so align down the requested seek position and seek to that.
                let aligned_n = self.align_down_to_sector_size(n);
                self.inner.seek(SeekFrom::Start(aligned_n))?;

                // Make the caller believe that we seeked to the actually requested position.
                // Our `read` implementation will cover the difference.
                self.stream_position = n;
                Ok(self.stream_position)
            }
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SectorReader;
    use std::io::{Cursor, Read, Seek, SeekFrom};

    fn pattern_data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// Contract: callers seek before every read (the ntfs crate does).
    fn read_at<R: Read + Seek>(
        r: &mut SectorReader<R>,
        pos: u64,
        n: usize,
    ) -> std::io::Result<Vec<u8>> {
        r.seek(SeekFrom::Start(pos))?;
        let mut out = vec![0u8; n];
        let got = r.read(&mut out)?;
        out.truncate(got);
        Ok(out)
    }

    #[test]
    fn new_rejects_non_power_of_two_sector_size() {
        assert!(SectorReader::new(Cursor::new(vec![]), 511).is_err());
        assert!(SectorReader::new(Cursor::new(vec![]), 0).is_err());
        assert!(SectorReader::new(Cursor::new(vec![]), 512).is_ok());
    }

    #[test]
    fn sequential_reads_with_unaligned_sizes_reassemble_source() {
        let data = pattern_data(1500);
        let mut sr = SectorReader::new(Cursor::new(data.clone()), 512).unwrap();
        let mut out = Vec::new();
        let mut pos = 0u64;
        loop {
            let got = read_at(&mut sr, pos, 400).unwrap();
            if got.is_empty() {
                break;
            }
            assert_eq!(got, data[pos as usize..pos as usize + got.len()]);
            pos += got.len() as u64;
            out.extend_from_slice(&got);
        }
        assert_eq!(out, data);
    }

    #[test]
    fn unaligned_middle_read_returns_exact_bytes() {
        let data = pattern_data(2048);
        let mut sr = SectorReader::new(Cursor::new(data.clone()), 512).unwrap();
        let got = read_at(&mut sr, 1000, 64).unwrap();
        assert_eq!(got, data[1000..1064]);
    }

    #[test]
    fn read_at_volume_end_returns_clean_eof() {
        let data = pattern_data(600);
        let mut sr = SectorReader::new(Cursor::new(data.clone()), 512).unwrap();
        // 512-aligned read starting past the last readable byte: EOF, not an error.
        let got = read_at(&mut sr, 600, 64).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn partial_read_at_volume_end_shortens() {
        let data = pattern_data(600);
        let mut sr = SectorReader::new(Cursor::new(data.clone()), 512).unwrap();
        // Requesting 100 bytes from offset 560 in a 600-byte volume yields exactly 40.
        let got = read_at(&mut sr, 560, 100).unwrap();
        assert_eq!(got, data[560..600]);
    }

    #[test]
    fn huge_stream_position_reads_as_clean_eof() {
        // Crafted filesystem metadata can place positions near u64::MAX; the reader must
        // report EOF rather than panic or overflow.
        let mut sr = SectorReader::new(Cursor::new(vec![0u8; 8]), 4096).unwrap();
        sr.seek(SeekFrom::Start(u64::MAX - 1)).unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(sr.read(&mut buf).unwrap(), 0);
    }

    #[test]
    fn seek_current_negative_overflow_is_error() {
        let mut sr = SectorReader::new(Cursor::new(vec![0u8; 8]), 512).unwrap();
        let res = sr.seek(SeekFrom::Current(i64::MIN));
        assert!(res.is_err());
    }
}

//! A bounded anonymous preview prefix with independent positional reads.
use crate::model::{MAX_RESULT_BYTES, MAX_RESULT_ROWS, Row};
use anyhow::Result;
use std::{
    fs::File,
    io::{BufWriter, Write},
};

pub(super) struct Prefix {
    writer: BufWriter<File>,
    reader: File,
    offset: u64,
    rows: usize,
    consumed: usize,
    bytes: usize,
    pub limited: bool,
}

impl Prefix {
    pub fn new() -> Result<Self> {
        let file = tempfile::tempfile()?;
        Ok(Self {
            reader: file.try_clone()?,
            writer: BufWriter::new(file),
            offset: 0,
            rows: 0,
            consumed: 0,
            bytes: 0,
            limited: false,
        })
    }

    pub fn available(&self) -> usize {
        self.rows - self.consumed
    }

    pub fn append(&mut self, row: Row) -> Result<()> {
        let bytes = row.len() * std::mem::size_of::<Option<String>>()
            + row.iter().flatten().map(String::capacity).sum::<usize>();
        if self.rows == MAX_RESULT_ROWS || self.bytes.saturating_add(bytes) > MAX_RESULT_BYTES {
            self.limited = true;
        }
        if self.limited {
            return Ok(());
        }
        self.writer.write_all(&(row.len() as u64).to_le_bytes())?;
        for value in row {
            match value {
                None => self.writer.write_all(&u64::MAX.to_le_bytes())?,
                Some(value) => {
                    self.writer.write_all(&(value.len() as u64).to_le_bytes())?;
                    self.writer.write_all(value.as_bytes())?;
                }
            }
        }
        self.rows += 1;
        self.bytes += bytes;
        Ok(())
    }

    pub fn next(&mut self, width: usize) -> Result<Option<Row>> {
        if self.available() == 0 {
            return Ok(None);
        }
        self.writer.flush()?;
        let mut size = [0; 8];
        self.read(&mut size)?;
        anyhow::ensure!(
            u64::from_le_bytes(size) as usize == width,
            "Invalid Trino preview row width"
        );
        let mut row = Vec::with_capacity(width);
        for _ in 0..width {
            self.read(&mut size)?;
            let length = u64::from_le_bytes(size);
            if length == u64::MAX {
                row.push(None);
            } else {
                anyhow::ensure!(
                    length <= crate::export::budget::MAX_ROW_BYTES as u64,
                    "Invalid Trino preview cell length"
                );
                let mut value = vec![0; length as usize];
                self.read(&mut value)?;
                row.push(Some(String::from_utf8(value)?));
            }
        }
        self.consumed += 1;
        Ok(Some(row))
    }

    fn read(&mut self, bytes: &mut [u8]) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.reader.read_exact_at(bytes, self.offset)?;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut read = 0;
            while read < bytes.len() {
                let count = self
                    .reader
                    .seek_read(&mut bytes[read..], self.offset + read as u64)?;
                anyhow::ensure!(count > 0, "Incomplete Trino preview record");
                read += count;
            }
        }
        self.offset += bytes.len() as u64;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_limit_stops_retention_and_preserves_the_prefix() -> Result<()> {
        let mut prefix = Prefix::new()?;
        prefix.bytes = MAX_RESULT_BYTES - 100;
        prefix.append(vec![Some("small".into())])?;
        prefix.append(vec![Some("x".repeat(100))])?;
        prefix.append(vec![Some("later".into())])?;
        assert!(prefix.limited);
        assert_eq!(prefix.available(), 1);
        assert_eq!(prefix.next(1)?, Some(vec![Some("small".into())]));
        assert!(prefix.next(1)?.is_none());
        assert_eq!(prefix.available(), 0);
        Ok(())
    }

    #[test]
    fn positional_reads_do_not_move_the_writer_or_lose_nulls() -> Result<()> {
        let mut prefix = Prefix::new()?;
        prefix.append(vec![None, Some(String::new())])?;
        assert_eq!(prefix.next(2)?, Some(vec![None, Some(String::new())]));
        prefix.append(vec![Some("α🦀".into()), None])?;
        assert_eq!(prefix.next(2)?, Some(vec![Some("α🦀".into()), None]));
        assert!(prefix.next(2)?.is_none());
        Ok(())
    }
}

// Copyright 2021 Colin Finck <colin@reactos.org>
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::fs::OpenOptions;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use log::debug;

use anyhow::{bail, Context, Result};
use ntfs::indexes::NtfsFileNameIndex;
use ntfs::structured_values::{NtfsFileName, NtfsFileNamespace};
use ntfs::{Ntfs, NtfsFile, NtfsReadSeek};

pub struct CommandInfo<'n, T>
where
    T: Read + Seek,
{
    pub current_directory: Vec<NtfsFile<'n>>,
    pub current_directory_string: String,
    pub fs: T,
    pub ntfs: &'n Ntfs,
}

fn best_file_name<T>(
    info: &mut CommandInfo<T>,
    file: &NtfsFile,
    parent_record_number: u64,
) -> Result<NtfsFileName>
where
    T: Read + Seek,
{
    // Try to find a long filename (Win32) first.
    // If we don't find one, the file may only have a single short name (Win32AndDos).
    // If we don't find one either, go with any namespace. It may still be a Dos or Posix name then.
    let priority = [
        Some(NtfsFileNamespace::Win32),
        Some(NtfsFileNamespace::Win32AndDos),
        None,
    ];

    for match_namespace in priority {
        if let Some(file_name) =
            file.name(&mut info.fs, match_namespace, Some(parent_record_number))
        {
            let file_name = file_name?;
            return Ok(file_name);
        }
    }

    bail!(
        "Found no FileName attribute for File Record {:#x}",
        file.file_record_number()
    )
}

pub fn cd<T>(arg: &str, info: &mut CommandInfo<T>) -> Result<()>
where
    T: Read + Seek,
{
    if arg.is_empty() {
        return Ok(());
    }

    if arg == ".." {
        if info.current_directory_string.is_empty() {
            return Ok(());
        }

        info.current_directory.pop();

        let new_len = info.current_directory_string.rfind('\\').unwrap_or(0);
        info.current_directory_string.truncate(new_len);
    } else {
        let index = info
            .current_directory
            .last()
            .unwrap()
            .directory_index(&mut info.fs)?;
        let mut finder = index.finder();
        let maybe_entry = NtfsFileNameIndex::find(&mut finder, info.ntfs, &mut info.fs, arg);

        if maybe_entry.is_none() {
            // Succeeding here made callers extract a same-named file from the
            // wrong directory - silent evidence corruption.
            bail!(
                "directory \"{arg}\" not found in raw-access path \"{}\"",
                info.current_directory_string
            );
        }

        let entry = maybe_entry.unwrap()?;
        let file_name = entry
            .key()
            .expect("key must exist for a found Index Entry")?;

        if !file_name.is_directory() {
            bail!("\"{arg}\" is not a directory");
        }

        let file = entry.to_file(info.ntfs, &mut info.fs)?;
        let file_name = best_file_name(
            info,
            &file,
            info.current_directory.last().unwrap().file_record_number(),
        )?;
        if !info.current_directory_string.is_empty() {
            info.current_directory_string += "\\";
        }
        info.current_directory_string += &file_name.name().to_string_lossy();

        info.current_directory.push(file);
    }

    Ok(())
}

pub fn get<T>(arg: &str, output_dir: &Path, info: &mut CommandInfo<T>) -> Result<PathBuf>
where
    T: Read + Seek,
{
    // Extract any specific $DATA stream name from the file.
    let (file_name, data_stream_name) = match arg.find(':') {
        Some(mid) => (&arg[..mid], &arg[mid + 1..]),
        None => (arg, ""),
    };

    // Stage under the caller-provided private directory (never the process
    // working directory: raw access is how SAM/SYSTEM get read on live hosts).
    // It must not yet exist, as we don't want to accidentally overwrite things.
    let output_file_name = if data_stream_name.is_empty() {
        file_name.to_string()
    } else {
        format!("{file_name}_{data_stream_name}")
    };
    let staged_path = output_dir.join(&output_file_name);
    let mut output_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged_path)
        .with_context(|| format!("Tried to open \"{output_file_name}\" for writing"))?;

    // Open the desired file and find the $DATA attribute we are looking for.
    let file = parse_file_arg(file_name, info)?;
    let data_item = match file.data(&mut info.fs, data_stream_name) {
        Some(data_item) => data_item,
        None => {
            bail!("The file does not have a \"{data_stream_name}\" $DATA attribute.")
        }
    };
    let data_item = data_item?;
    let data_attribute = data_item.to_attribute();
    let mut data_value = data_attribute.value(&mut info.fs)?;

    debug!(
        "raw-extracting {} bytes of data from \"{file_name}\"",
        data_value.len()
    );
    let mut buf = vec![0u8; 256 * 1024];

    loop {
        let bytes_read = data_value.read(&mut info.fs, &mut buf)?;
        if bytes_read == 0 {
            break;
        }
        output_file.write_all(&buf[..bytes_read])?;
    }

    Ok(staged_path)
}

fn parse_file_arg<'n, T>(arg: &str, info: &mut CommandInfo<'n, T>) -> Result<NtfsFile<'n>>
where
    T: Read + Seek,
{
    if arg.is_empty() {
        bail!("Missing argument!");
    }

    if let Some(record_number_arg) = arg.strip_prefix("/") {
        let record_number = match record_number_arg.strip_prefix("0x") {
            Some(hex_record_number_arg) => u64::from_str_radix(hex_record_number_arg, 16),
            None => u64::from_str_radix(record_number_arg, 10),
        };

        if let Ok(record_number) = record_number {
            let file = info.ntfs.file(&mut info.fs, record_number)?;
            Ok(file)
        } else {
            bail!(
                "Cannot parse record number argument \"{}\"",
                record_number_arg
            )
        }
    } else {
        let index = info
            .current_directory
            .last()
            .unwrap()
            .directory_index(&mut info.fs)?;
        let mut finder = index.finder();

        if let Some(entry) = NtfsFileNameIndex::find(&mut finder, info.ntfs, &mut info.fs, arg) {
            let entry = entry?;
            let file = entry.to_file(info.ntfs, &mut info.fs)?;
            Ok(file)
        } else {
            bail!("No such file or directory \"{}\".", arg)
        }
    }
}

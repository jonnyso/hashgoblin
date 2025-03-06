use jiff::{Unit, Zoned};

use crate::{Error, hashing::HashType, verbose_print};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Seek, Write},
    path::Path,
    sync::Mutex,
};

use super::{
    HASH_ALGO_STR, HashData, HashHandler, NO_DATE_STR, TIME_FINISH_STR, TIME_START_STR, VERSION_STR,
};

type GuardedWriter = Mutex<BufWriter<File>>;

pub struct OutFile {
    writer: GuardedWriter,
}

impl OutFile {
    pub fn new(path: &Path, hash: &[HashType]) -> Result<Self, Error> {
        verbose_print("creating output file", true);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(Error::OutputRead)?;
        let mut writer = BufWriter::new(file);
        let version = env!("CARGO_PKG_VERSION");
        let time = current_time_string();
        let mut time_str: Vec<u8> = format!(
            "{VERSION_STR} {version}\n{HASH_ALGO_STR} {}\n{TIME_START_STR} {time} - {TIME_FINISH_STR} ",
            hash.iter().map(HashType::as_str).collect::<Vec<&str>>().join(",")
        )
        .into();
        time_str.extend(vec![b' '; time.len()]);
        time_str.push(b'\n');
        writer.write_all(&time_str).map_err(Error::OutputWrite)?;
        Ok(Self {
            writer: Mutex::new(writer),
        })
    }

    pub fn writer(&self) -> &GuardedWriter {
        &self.writer
    }

    pub fn finish(self) -> Result<(), Error> {
        verbose_print("writing finish date", true);
        let writer = self.writer.into_inner().map_err(|_| {
            Error::OutputFinish("failed retrieve outfile bufwriter out of mutex".to_owned())
        })?;
        let mut file = writer.into_inner().map_err(|_| {
            Error::OutputFinish("failed to retrieve inner file out of bufwriter".to_owned())
        })?;
        file.rewind().unwrap();
        let time_str = current_time_string();
        let cursor = {
            let mut reader = BufReader::new(&file);
            let _ = reader.by_ref().lines().nth(2);
            reader.stream_position().unwrap() - (time_str.len() + 1) as u64
        };

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::FileExt;

            file.seek_write(time_str.as_bytes(), cursor)
                .map_err(Error::OutputWrite)?;
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::FileExt;

            file.write_at(time_str.as_bytes(), cursor)
                .map_err(Error::OutputWrite)?;
        }

        Ok(())
    }
}

impl HashHandler for &GuardedWriter {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        self.lock()
            .unwrap()
            .write_all(format!("{hash_data}\n").as_bytes())
            .map_err(Error::OutputWrite)
    }
}

fn current_time_string() -> String {
    match Zoned::now().round(Unit::Second) {
        Ok(dt) => dt.datetime().to_string(),
        Err(err) => {
            eprintln!("WARNING: failed to aquire current date: {err}");
            NO_DATE_STR.to_owned()
        }
    }
}

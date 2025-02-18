use jiff::{Unit, Zoned};

use crate::{hashing::HashType, Error, DEFAULT_OUT};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::{HashData, HashHandler};

pub struct OutFile {
    writer: Mutex<BufWriter<File>>,
    path: PathBuf,
}

impl OutFile {
    pub fn new(path: Option<PathBuf>, hash: &HashType) -> Result<Self, Error> {
        let path = path.unwrap_or(PathBuf::from(DEFAULT_OUT));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(Error::Output)?;
        let mut writer = BufWriter::new(file);
        let version = env!("CARGO_PKG_VERSION");
        let time = current_time_string();
        writer
            .write_all(format!("version {version}\nalgo {hash}\ntime_start {time}\n").as_bytes())
            .map_err(Error::Output)?;
        Ok(Self {
            writer: Mutex::new(writer),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl HashHandler for OutFile {
    fn handle(&self, hash_data: HashData) -> Result<(), Error> {
        let line = format!("{hash_data}\n");
        self.writer
            .lock()
            .unwrap()
            .write_all(line.as_bytes())
            .map_err(Error::Output)
    }
}

fn current_time_string() -> String {
    match Zoned::now().round(Unit::Second) {
        Ok(dt) => dt.datetime().to_string(),
        Err(err) => {
            eprintln!("WARNING: failed to aquire current date: {err}");
            "[NO DATE]".to_owned()
        }
    }
}

pub fn add_time_finish(path: &Path) -> io::Result<()> {
    let outfile = OpenOptions::new().read(true).write(true).open(path)?;
    let position = {
        let mut reader = BufReader::new(&outfile);
        let _ = reader.by_ref().lines().skip(2).next();
        reader.seek(SeekFrom::Current(0))? - 1
    };
    let mut time_str: Vec<u8> = format!(" - time_finish {}", current_time_string()).into();
    let mut overlap = vec![0; time_str.len()];
    outfile.read_at(&mut overlap, position)?;
    time_str.extend(overlap);
    outfile.write_at(&time_str, position)?;
    Ok(())
}

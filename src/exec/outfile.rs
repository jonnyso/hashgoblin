use crate::{hashing::HashType, Error, DEFAULT_OUT};
use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
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
        writer
            .write_all(format!("version {version}\nalgo {hash}\n").as_bytes())
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

use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
    sync::Mutex,
};

use crate::Error;

use super::{
    cancel_on_err, path_string, push_dir, ExecReaderHandle, ReadData, BUF_SIZE, HANDLE_BUF_SIZE,
};

enum NewReader {
    File(PathBuf, BufReader<File>),
    Dir(PathBuf),
}

pub struct MTReader {
    paths: VecDeque<PathBuf>,
}

impl MTReader {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: VecDeque::from(paths),
        }
    }

    fn new_reader(&mut self) -> Result<Option<NewReader>, Error> {
        match self.paths.pop_front() {
            Some(path) if path.is_dir() => Ok(Some(NewReader::Dir(path))),
            Some(path) => {
                let file = cancel_on_err(File::open(&path))
                    .map_err(|err| Error::Io((err, path_string(&path))))?;
                let reader = BufReader::with_capacity(BUF_SIZE, file);
                Ok(Some(NewReader::File(path, reader)))
            }
            None => Ok(None),
        }
    }
}

struct MTReaderHandle<'a> {
    queue: &'a Mutex<MTReader>,
    reader: Option<(PathBuf, BufReader<File>)>,
}

impl<'a> MTReaderHandle<'a> {
    pub fn new(mtreader: &'a Mutex<MTReader>) -> Self {
        Self {
            queue: mtreader,
            reader: None,
        }
    }
}

impl ExecReaderHandle for MTReaderHandle<'_> {
    fn try_read(&mut self) -> Result<Option<ReadData>, Error> {
        loop {
            break match self.reader.as_mut() {
                Some((path, reader)) => {
                    let mut buf = [0; HANDLE_BUF_SIZE];
                    let byte_count = cancel_on_err(reader.read(&mut buf))
                        .map_err(|err| Error::Io((err, path_string(&path))))?;
                    if byte_count == 0 {
                        self.reader = None;
                        Ok(None)
                    } else {
                        Ok(Some(ReadData::File(Some(buf))))
                    }
                }
                None => {
                    let mut queue = self.queue.lock().unwrap();
                    match cancel_on_err(queue.new_reader())? {
                        Some(NewReader::Dir(path)) => {
                            if !push_dir(&path, &mut queue.paths)? {
                                continue;
                            }
                            Ok(Some(ReadData::EmptyDir(path)))
                        }
                        Some(NewReader::File(path, reader)) => {
                            self.reader = Some((path, reader));
                            continue;
                        }
                        None => Ok(None),
                    }
                }
            };
        }
    }
}

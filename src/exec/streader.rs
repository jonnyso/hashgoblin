use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
    sync::Mutex,
};

use crate::Error;

use super::{
    cancel_on_err, path_string, push_dir, ExecReader, ExecReaderHandle, ReadData, BUF_SIZE,
    HANDLE_BUF_SIZE,
};

enum NewReader {
    EmptyDir(PathBuf),
    File(PathBuf, BufReader<File>),
}

type ReaderItem = (PathBuf, BufReader<File>);

pub struct STReader {
    paths: VecDeque<PathBuf>,
    readers: Vec<Option<ReaderItem>>,
}

impl STReader {
    pub fn new(paths: Vec<PathBuf>, max_threads: u8) -> Self {
        Self {
            paths: VecDeque::from(paths),
            readers: Vec::with_capacity(max_threads as usize),
        }
    }

    fn get_new_reader(&mut self) -> Result<Option<NewReader>, Error> {
        while let Some(path) = self.paths.pop_front() {
            if path.is_dir() {
                if push_dir(&path, &mut self.paths)? {
                    return Ok(Some(NewReader::EmptyDir(path)));
                }
                continue;
            }
            let file = cancel_on_err(File::open(&path))
                .map_err(|err| Error::Io((err, path_string(&path))))?;
            let reader = BufReader::with_capacity(BUF_SIZE, file);
            return Ok(Some(NewReader::File(path, reader)));
        }
        Ok(None)
    }

    fn at_mut(&mut self, index: usize) -> Option<&mut ReaderItem> {
        self.readers[index].as_mut()
    }

    fn set_at(&mut self, index: usize, value: Option<ReaderItem>) {
        self.readers[index] = value;
    }

    fn take_at(&mut self, index: usize) -> Option<ReaderItem> {
        self.readers[index].take()
    }
}

pub struct STReaderHandle<'a> {
    index: usize,
    inner: &'a Mutex<STReader>,
    buf: [u8; HANDLE_BUF_SIZE],
}

impl<'a> STReaderHandle<'a> {
    pub fn new(streader: &'a Mutex<STReader>) -> Self {
        let index = {
            let mut reader = streader.lock().unwrap();
            reader.readers.push(None);
            reader.readers.len() - 1
        };
        Self {
            index,
            inner: streader,
            buf: [0; HANDLE_BUF_SIZE],
        }
    }
}

impl ExecReaderHandle for STReaderHandle<'_> {
    fn try_read(&mut self) -> Result<Option<ReadData>, Error> {
        let mut locked = self.inner.lock().unwrap();
        loop {
            break match locked.at_mut(self.index) {
                Some((path, reader)) => {
                    let byte_count = cancel_on_err(reader.read(&mut self.buf))
                        .map_err(|err| Error::Io((err, path_string(path))))?;
                    if byte_count == 0 {
                        let (path, _) = locked.take_at(self.index).unwrap();
                        Ok(Some(ReadData::FileDone(path)))
                    } else {
                        Ok(Some(ReadData::OpenFile(&self.buf[..byte_count])))
                    }
                }
                None => match locked.get_new_reader()? {
                    Some(NewReader::EmptyDir(path)) => Ok(Some(ReadData::EmptyDir(path))),
                    Some(NewReader::File(path, reader)) => {
                        locked.set_at(self.index, Some((path, reader)));
                        continue;
                    }
                    None => Ok(None),
                },
            };
        }
    }
}

impl ExecReader for Mutex<STReader> {
    fn get_handle(&self) -> impl ExecReaderHandle {
        STReaderHandle::new(self)
    }
}

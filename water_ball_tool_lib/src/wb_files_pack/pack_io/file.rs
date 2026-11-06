use std::fmt::Debug;
use std::fs::File;
use std::io;
use std::io::{Read, Seek, Write};

pub trait FileIo: Debug + Write + Read + Seek + Send {
    fn set_len(&self, len: u64) -> io::Result<()>;
    fn sync_data(&self) -> io::Result<()>;
    fn lock(&self) -> io::Result<()>;
    fn unlock(&self) -> io::Result<()>;
}

impl FileIo for File {
    fn set_len(&self, len: u64) -> io::Result<()> {
        File::set_len(self, len)
    }
    fn sync_data(&self) -> io::Result<()> {
        File::sync_data(self)
    }
    fn lock(&self) -> io::Result<()> {
        File::lock(self)
    }
    fn unlock(&self) -> io::Result<()> {
        File::unlock(self)
    }
}

use std::fs::File;
use std::io::{Error, ErrorKind, Read, Write};
use std::path::PathBuf;
use std::{fs, io};
use tracing::info;

use super::{PackLockInfo, PackLockType, WBFPManager};

impl WBFPManager {
    fn this_write_lock_info(&self) -> PackLockInfo {
        let run_lock = self.run_data.write_lock;
        let path = &self.run_data.write_lock_path;
        Self::write_lock_info(run_lock, path)
    }

    pub(crate) fn this_write_lock(&mut self) -> io::Result<()> {
        if !self.run_data.write_lock {
            let pack_file = self.pack_file.clone();
            let mut pack_file = pack_file
                .lock()
                .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
            let lock_file = Self::write_lock(true, &self.run_data.write_lock_path)?;
            if let Some(lock_file) = lock_file {
                self.run_data.write_lock_file = Some(lock_file);
            }
            self.run_data.write_lock = true;
            pack_file.lock()?;
        }
        Ok(())
    }

    pub(super) fn write_unlock(&mut self) -> io::Result<()> {
        let pack_file = self.pack_file.clone();
        let mut pack_file = pack_file
            .lock()
            .map_err(|err| Error::other(format!("无法获得包文件锁, err: {err}")))?;
        let path = &self.run_data.write_lock_path;
        let lock_info = self.this_write_lock_info();
        match lock_info.file_lock_type {
            PackLockType::File => {
                if let Some(lock_file) = self.run_data.write_lock_file.take() {
                    lock_file.unlock()?;
                    drop(lock_file);
                    fs::remove_file(path)?;
                }
                self.run_data.write_lock = false;
                pack_file.unlock()?;
                Ok(())
            }
            PackLockType::Dir => Err(Error::new(
                ErrorKind::IsADirectory,
                "无法解锁，锁文件类型很可能已被其他程序修改成目录",
            ))?,
            PackLockType::Symlink => Err(Error::other(
                "无法解锁，锁文件类型很可能已被其他程序修改成符号链接",
            ))?,
            PackLockType::_None => Ok(()),
        }
    }

    pub(super) fn write_lock_info(run_lock: bool, path: &PathBuf) -> PackLockInfo {
        fn is_process_running(pid: u32, system: &sysinfo::System) -> bool {
            system.process(sysinfo::Pid::from(pid as usize)).is_some()
        }
        let system = sysinfo::System::new_all();
        let is_symlink = path.is_symlink();
        let is_dir;
        let mut file_lock_pid = None;
        let mut file_lock_pid_run = None;
        if path.try_exists().is_ok() {
            is_dir = path.is_dir();
            if path.is_file()
                && !run_lock
                && let Ok(mut file) = File::open(path)
            {
                let mut buf = [0u8; 4];
                if file.read_exact(&mut buf).is_ok() {
                    let pid = u32::from_le_bytes(buf);
                    file_lock_pid = Some(pid);
                    file_lock_pid_run = Some(is_process_running(pid, &system));
                }
            }
        } else {
            is_dir = false;
        }
        let file_lock_type = if is_symlink {
            PackLockType::Symlink
        } else if is_dir {
            PackLockType::Dir
        } else {
            PackLockType::File
        };
        PackLockInfo {
            run_lock,
            file_lock_type,
            file_lock_pid,
            file_lock_pid_run,
        }
    }

    pub(super) fn write_lock(
        run_lock: bool,
        write_lock_path: &PathBuf,
    ) -> io::Result<Option<File>> {
        let lock_info = Self::write_lock_info(run_lock, write_lock_path);
        if lock_info.run_lock {
            if let PackLockType::_None = lock_info.file_lock_type {
                Ok(Some(Self::write_lock_file(write_lock_path)?))
            } else {
                Ok(None)
            }
        } else {
            match lock_info.file_lock_pid_run {
                Some(true) => panic!("无法为包文件上写入锁，正在被其他进程持有。"),
                Some(false) => panic!(
                    r#"包文件未正常解锁，但相关进程(pid:{})可能已停止。如果你认为可以继续，可以删除锁文件"{}"强制解锁"#,
                    lock_info.file_lock_pid.expect("pid参数不存在"),
                    write_lock_path.display()
                ),
                None => Ok(Some(Self::write_lock_file(write_lock_path)?)),
            }
        }
    }

    pub(super) fn write_lock_file(write_lock_path: &PathBuf) -> Result<File, Error> {
        let pid = std::process::id();
        let mut write_lock = File::create(write_lock_path)?;
        write_lock.write_all(pid.to_le_bytes().as_slice())?;
        write_lock.sync_all()?;
        write_lock.lock()?;
        info!("已为包文件上写入锁");
        Ok(write_lock)
    }
}

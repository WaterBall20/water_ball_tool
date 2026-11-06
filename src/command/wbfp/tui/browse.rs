//! wbfp TUI 目录浏览 / wbfp TUI directory browsing
//!
//! 包内目录通过 `ManagerSync` 懒加载；打包模式浏览本地源目录。
//!
//! Pack directories are loaded lazily through `ManagerSync`; pack mode browses
//! the local source directory instead.

//AI=== 包内/本地目录读取与条目模型 / pack/local directory loading and the entry model
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;
use water_ball_tool_lib::wb_files_pack::{
    PackFileMetadataRun, PackFileMetadataType, PackStructItem, PackStructItemType,
};

/// 列表中的一行 / one row in the file list.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub name: String,
    /// 状态键：包内相对路径或本地完整路径 / status key: pack-relative or full local path
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// 修改时间（毫秒）/ modification time in milliseconds
    pub modified: u128,
    /// 文件哈希（类型，值）/ file hash (type, value)
    pub hash: Option<(u8, Vec<u8>)>,
    /// `..` 行 / the `..` row
    pub synthetic: bool,
}

impl Entry {
    fn parent(path: PathBuf) -> Self {
        Self {
            name: "..".to_string(),
            path,
            is_dir: true,
            size: 0,
            modified: 0,
            hash: None,
            synthetic: true,
        }
    }
}

/// 包属性摘要 / pack attribute summary.
#[derive(Clone, Debug, Default)]
pub(crate) struct PackInfo {
    pub version: u16,
    pub file_count: u64,
    pub dir_count: u64,
    pub data_len: u64,
    pub cow: bool,
}

/// 读取包全局属性 / read the pack's global attributes.
pub(crate) fn pack_info(pack: &ManagerSync) -> Result<PackInfo, String> {
    let a = pack
        .get_manifest_attribute()
        .map_err(|e| format!("读取包属性失败 / failed to read pack attributes: {e}"))?;
    Ok(PackInfo {
        version: a.version(),
        file_count: a.file_count(),
        dir_count: a.dir_count(),
        data_len: a.data_len(),
        cow: a.cow(),
    })
}

/// 读取包内某个目录 / load one directory inside the pack.
pub(crate) fn pack_dir(pack: &mut ManagerSync, cwd: &Path) -> Result<Vec<Entry>, String> {
    let names = if cwd.as_os_str().is_empty() {
        pack.get_root_struct_item_name_list()
            .map_err(|e| format!("读取包根目录失败 / failed to read pack root: {e}"))?
    } else {
        pack.get_struct_item_name_list(&cwd)
            .map_err(|e| format!("读取目录失败 / failed to read directory: {e}"))?
    };
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let path = if cwd.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            cwd.join(&name)
        };
        let item = pack.get_pack_struct_item(&path).map_err(|e| {
            format!(
                "读取条目失败 / failed to read entry {}: {e}",
                path.display()
            )
        })?;
        entries.push(pack_entry(name, path, &item));
    }
    sort_entries(&mut entries);
    Ok(entries)
}

fn pack_entry(name: String, path: PathBuf, item: &PackStructItem) -> Entry {
    let is_dir = matches!(item.item_type(), PackStructItemType::Dir { .. });
    let mut size = 0;
    let mut modified = 0;
    let mut hash = None;
    if let PackFileMetadataRun::Loaded(meta) = item.metadata() {
        size = meta.len();
        modified = meta.modified();
        if let PackFileMetadataType::File {
            hash_type,
            hash_value,
            ..
        } = meta.file_type()
        {
            hash = Some((*hash_type, hash_value.clone()));
        }
    }
    Entry {
        name,
        path,
        is_dir,
        size,
        modified,
        hash,
        synthetic: false,
    }
}

/// 读取本地源目录 / load a local source directory.
pub(crate) fn local_dir(cwd: &Path) -> Result<Vec<Entry>, String> {
    let iter = fs::read_dir(cwd).map_err(|e| {
        format!(
            "读取本地目录失败 / failed to read local directory {}: {e}",
            cwd.display()
        )
    })?;
    let mut entries = Vec::new();
    for item in iter {
        let item =
            item.map_err(|e| format!("读取目录项失败 / failed to read directory entry: {e}"))?;
        let file_type = item
            .file_type()
            .map_err(|e| format!("读取文件类型失败 / failed to read file type: {e}"))?;
        if file_type.is_symlink() || (!file_type.is_file() && !file_type.is_dir()) {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        let path = item.path();
        let metadata = item
            .metadata()
            .map_err(|e| format!("读取文件信息失败 / failed to stat {}: {e}", path.display()))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis())
            .unwrap_or(0);
        entries.push(Entry {
            name,
            path,
            is_dir: file_type.is_dir(),
            size: metadata.len(),
            modified,
            hash: None,
            synthetic: false,
        });
    }
    sort_entries(&mut entries);
    Ok(entries)
}

/// 单个本地文件作为一个条目 / expose one local file as an entry.
pub(crate) fn local_file(path: &Path) -> Result<Entry, String> {
    let metadata = path
        .metadata()
        .map_err(|e| format!("读取文件信息失败 / failed to stat {}: {e}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(Entry {
        name: path
            .file_name()
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        path: path.to_path_buf(),
        is_dir: false,
        size: metadata.len(),
        modified,
        hash: None,
        synthetic: false,
    })
}

/// 插入 `..` 行 / prepend the `..` row.
pub(crate) fn with_parent(mut entries: Vec<Entry>, parent: PathBuf) -> Vec<Entry> {
    entries.insert(0, Entry::parent(parent));
    entries
}

fn sort_entries(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// 毫秒时间戳格式化为本地时间 / format a millisecond timestamp as local time.
pub(crate) fn format_modified(modified: u128) -> String {
    if modified == 0 {
        return "—".to_string();
    }
    let Ok(ms) = i64::try_from(modified) else {
        return "—".to_string();
    };
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string())
}

/// 哈希显示文本 / hash display text.
pub(crate) fn format_hash(hash: Option<&(u8, Vec<u8>)>) -> String {
    let Some((kind, value)) = hash else {
        return "—".to_string();
    };
    let name = match kind {
        1 => "blake3",
        _ => "unknown",
    };
    let mut hex = String::with_capacity(value.len() * 2);
    for b in value {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    if hex.len() > 16 {
        format!("{name} {}…{}", &hex[..8], &hex[hex.len() - 8..])
    } else {
        format!("{name} {hex}")
    }
}
//AI_END===

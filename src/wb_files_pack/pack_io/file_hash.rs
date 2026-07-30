use crate::wb_files_pack::error::{PackFileError, Result};
use blake3::{Hash, Hasher};

/// 包文件哈希状态 / Pack file hash state
#[derive(Debug, Clone)]
pub(crate) enum PackFileHash {
    /// 未计算哈希 / No hash computed
    None,
    /// Blake3 哈希 / Blake3 hash
    Blake3 {
        /// 哈希计算器 / Hash hasher
        hasher: Box<Hasher>,
        /// 哈希值 / Hash value
        hash_value: Vec<u8>,
    },
}

impl PackFileHash {
    pub(crate) fn new(type_: u8) -> Self {
        match type_ {
            1 => Self::Blake3 {
                hasher: Box::new(Hasher::new()),
                hash_value: Vec::new(),
            },
            _ => Self::None,
        }
    }
    pub(crate) fn from_new(type_: u8, value: &[u8]) -> Self {
        match type_ {
            1 => Self::Blake3 {
                hasher: Box::new(Hasher::new()),
                hash_value: value.to_vec(),
            },
            _ => Self::None,
        }
    }
}

impl PackFileHash {
    fn _to_u8_type(&self) -> u8 {
        match self {
            Self::None => 0,
            Self::Blake3 { .. } => 1,
        }
    }

    pub(crate) fn update(&mut self, input: &[u8]) {
        match self {
            PackFileHash::Blake3 { hasher, .. } => {
                hasher.update(input);
            }
            PackFileHash::None => (),
        }
    }

    pub(crate) fn get_hash_value(&self) -> Vec<u8> {
        match self {
            Self::None => Vec::new(),
            Self::Blake3 { hasher, .. } => {
                let this_hash = hasher.finalize();
                this_hash.as_bytes().to_vec()
            }
        }
    }

    pub(crate) fn eq(&mut self, other: &PackFileHash) -> Result<bool> {
        match self {
            Self::None => Err(PackFileError::State("无法对没有哈希计算的进行比较".into())),
            Self::Blake3 { hasher, .. } => {
                if let Self::Blake3 { hash_value, .. } = other {
                    let hash = hasher.finalize();
                    let other_hash = Hash::from_slice(hash_value)
                        .map_err(|e| PackFileError::Format(format!("比较发生错误，err: {e:?}")))?;
                    Ok(hash == other_hash)
                } else {
                    Err(PackFileError::State("不能对不同类型进行比较".into()))
                }
            }
        }
    }
}

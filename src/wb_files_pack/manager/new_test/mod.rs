/*
创建：2026/02/27
 */
//新功能测试

use crate::wb_files_pack::{Attribute, PackFilesList, WBFilesPacManifest};

//二进制索引文件格式创建测试
#[test]
fn index_data_1() {
    //索引实例
    let data: WBFilesPacManifest = WBFilesPacManifest {
        attribute: Attribute::default(),
        pack_files_list: PackFilesList {
            file_count: 11,
            dir_count: 10,
            ..PackFilesList::default()
        },
    };
    let mut attribute_data: Vec<u8> = Vec::with_capacity(2 + 2 + 1 + 8 + 8 + 8);
    //格式版本
    for to_le_byte in data.attribute.data_version.value.to_le_bytes() {
        attribute_data.push(to_le_byte);
    }
    //兼容版本
    for to_le_byte in data.attribute.data_version.compatible().to_le_bytes() {
        attribute_data.push(to_le_byte);
    }
    //布尔数据
    let mut bool_data = 0u8;
    //写时复制
    if data.attribute().cow {
        bool_data |= 0b10000000;
    }
    //写入
    attribute_data.push(bool_data);
    //空数据集合文件指针位置
    for i in [0u8; 8] {
        attribute_data.push(i);
    }
    //所有文件数
    for to_le_byte in data.pack_files_list.file_count.to_le_bytes() {
        attribute_data.push(to_le_byte);
    }
    //所有目录数
    for to_le_byte in data.pack_files_list.dir_count.to_le_bytes() {
        attribute_data.push(to_le_byte);
    }
    assert_eq!(
        attribute_data,
        vec![
            10, 0, //格式版本
            10, 0, //格式兼容版本
            0, //布尔数据
            0, 0, 0, 0, 0, 0, 0, 0, //空数据集合文件指针位置
            11, 0, 0, 0, 0, 0, 0, 0, //所有文件数
            10, 0, 0, 0, 0, 0, 0, 0 //所有目录数
        ]
    )
}

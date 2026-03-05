use crate::wb_files_pack::{
    Attribute,
    DataPosList,
    ManifestDataBlock,
    PackFileMetadata,
    PackFileMetadataDir,
    PackFileMetadataFile,
    PackFileMetadataType,
    PackStruct,
    PackStructItem,
    PackStructItemDir,
    PackStructItemType,
};

#[test]
fn pack_struct_to_bytes_vec_and_load() {
    let mut ps = PackStruct::default();
    ps.items.insert("test".to_string(), PackStructItem {
        name: "test".to_string(),
        metadata_file_pos: 5867,
        item_type: PackStructItemType::Dir(PackStructItemDir {
            struct_file_pos: 2894,
            pack_struct: None,
        }),
        pack_file_metadata: None,
    });
    ps.items.insert("test2".to_string(), PackStructItem {
        name: "test2".to_string(),
        metadata_file_pos: 2941,
        item_type: PackStructItemType::Dir(PackStructItemDir {
            struct_file_pos: 2984,
            pack_struct: None,
        }),
        pack_file_metadata: None,
    });
    let data = ps.get_bytes_vec();
    let ps_load = PackStruct::load(0, &data).unwrap();
    assert_eq!(ps, ps_load);
}
#[test]
fn pack_struct_item_to_bytes_vec_and_load() {
    let psi = PackStructItem {
        name: String::new(),
        pack_file_metadata: None,
        item_type: PackStructItemType::Dir(PackStructItemDir::default()),
        metadata_file_pos: 0,
    };
    let data = psi.to_bytes_vec();
    let psi_load = PackStructItem::load(&data).unwrap();
    assert_eq!(psi, psi_load);
}

#[test]
fn pack_file_metadata_file_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata {
        data_len: 0,
        cow: true,
        len: 53124,
        modified: 5715,
        file_type: PackFileMetadataType::File(PackFileMetadataFile {
            hash_type: 0,
            hash: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            data_pos_list: DataPosList {
                data_len: 0,
                list: vec![(0, 100), (10, 1), (223, 5890)],
            },
        }),
    };
    let data = pfm.get_bytes_vec();
    let pfm_load = PackFileMetadata::load(&data).unwrap();
    assert_eq!(pfm, pfm_load);
}
#[test]
fn pack_file_metadata_dir_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata {
        data_len: 0,
        cow: true,
        len: 52035,
        modified: 294,
        file_type: PackFileMetadataType::Dir(PackFileMetadataDir::default()),
    };
    let data = pfm.get_bytes_vec();
    let pfm_load = PackFileMetadata::load(&data).unwrap();
    assert_eq!(pfm, pfm_load);
}

#[test]
fn pack_file_metadata_data_block_save_and_load() {
    let mut a = Attribute::default();
    let a_data = a.get_bytes_vec();
    let mut data_block = vec![0u8; ManifestDataBlock::get_block_len_us(a_data.len())];
    //Save
    ManifestDataBlock::save_data_to_block_data_new(&a_data, &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let a_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(a, a_load);
    //Save2
    let mut b = Attribute {
        cow: true,
        root_struct_pos: 231,
        empty_data_pos_list_pos: 255,
        file_count: 99,
        dir_count: 877,
        version: 10,
        version_compatible: 10,
    };
    ManifestDataBlock::save_data_to_block_data(&b.get_bytes_vec(), &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let b_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(b, b_load);
    //Save3
    ManifestDataBlock::save_data_to_block_data(&b.get_bytes_vec(), &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let b_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(b, b_load);
}

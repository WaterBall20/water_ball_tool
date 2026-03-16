use crate::wb_files_pack::{
    Attribute, DataPosList, ManifestDataBlock, PackFileMetadata, PackFileMetadataRun,
    PackFileMetadataType, PackStruct, PackStructItem, PackStructItemDir, PackStructItemType,
};

#[test]
fn pack_struct_to_bytes_vec_and_load() {
    let mut ps = PackStruct::default();
    ps.items.insert(
        "test".to_string(),
        PackStructItem {
            name: "test".to_string(),
            metadata_file_pos: 5867,
            item_type: PackStructItemType::Dir(PackStructItemDir {
                struct_file_pos: 2894,
                pack_struct: None,
            }),
            metadata: PackFileMetadataRun::NoLoad,
        },
    );
    ps.items.insert(
        "test2".to_string(),
        PackStructItem {
            name: "test2".to_string(),
            metadata_file_pos: 2941,
            item_type: PackStructItemType::Dir(PackStructItemDir {
                struct_file_pos: 2984,
                pack_struct: None,
            }),
            metadata: PackFileMetadataRun::NoLoad,
        },
    );
    let block_data = ManifestDataBlock::from_block_data_new(ps.get_block_data().0, 0).unwrap();
    let ps_load = PackStruct::load(block_data).unwrap();
    assert_eq!(ps, ps_load);
}
#[test]
fn pack_struct_item_dir_to_bytes_vec_and_load() {
    let psi = PackStructItem {
        name: String::new(),
        metadata: PackFileMetadataRun::NoLoad,
        item_type: PackStructItemType::Dir(PackStructItemDir::default()),
        metadata_file_pos: 7_766_735_636,
    };
    let data = psi.to_bytes_vec();
    let psi_load = PackStructItem::load(&data).unwrap();
    assert_eq!(psi, psi_load);
}
#[test]
fn pack_struct_item_file_to_bytes_vec_and_load() {
    let psi = PackStructItem {
        name: String::new(),
        metadata: PackFileMetadataRun::NoLoad,
        item_type: PackStructItemType::File,
        metadata_file_pos: 689_669,
    };
    let data = psi.to_bytes_vec();
    let psi_load = PackStructItem::load(&data).unwrap();
    assert_eq!(psi, psi_load);
}
#[test]
fn pack_file_metadata_file_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata {
        data_block: ManifestDataBlock::default(),
        cow: false,
        len: 53124,
        modified: 5715,
        file_type: PackFileMetadataType::File {
            hash_type: 0,
            hash_value: Vec::new(),
            data_pos_list: DataPosList {
                data_block: None,
                list: vec![(0, 100), (10, 1), (223, 5890)],
            },
        },
    };
    let hash_value = vec![
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
        26, 27, 28, 29, 30, 31, 32,
    ];
    let pfm2 = PackFileMetadata {
        file_type: PackFileMetadataType::File {
            hash_type: 1,
            hash_value: hash_value.clone(),
            data_pos_list: DataPosList {
                data_block: None,
                list: vec![(0, 100), (10, 1), (223, 5890)],
            },
        },
        ..pfm.clone()
    };
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    assert_eq!(pfm, pfm_load);
    //2
    if let PackFileMetadataType::File {
        hash_type,
        hash_value: this_hash_value,
        ..
    } = &mut pfm.file_type
    {
        *hash_type = 1;
        *this_hash_value = hash_value
    }
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    assert_eq!(pfm2, pfm_load);
}
#[test]
fn pack_file_metadata_dir_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata {
        data_block: ManifestDataBlock::default(),
        cow: true,
        len: 52035,
        modified: 294,
        file_type: PackFileMetadataType::Dir {
            file_count: 0,
            dir_count: 0,
        },
    };
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    assert_eq!(pfm, pfm_load);
}

#[test]
fn pack_file_metadata_data_block_save_and_load() {
    let a = Attribute::default();
    let a_data = a.to_bytes_vec();
    let mut data_block = vec![0u8; ManifestDataBlock::get_block_len_us(a_data.len())];
    //Save
    ManifestDataBlock::save_data_to_block_data_new(&a_data, &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let a_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(a, a_load);
    //Save2
    let b = Attribute {
        version: 10,
        version_compatible: 10,
        cow: true,
        file_count: 99,
        dir_count: 877,
        data_len: 231,
        root_struct_pos: 231,
        empty_data_pos_list_pos: 255,
        manifest_empty_data_pos_list_pos: 241,
        manifest_file_len: 123,
    };
    ManifestDataBlock::save_data_to_block_data(&b.to_bytes_vec(), &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let b_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(b, b_load);
    //Save3
    ManifestDataBlock::save_data_to_block_data(&b.to_bytes_vec(), &mut data_block).unwrap();
    //Load
    let save_load_data = ManifestDataBlock::get_data(&data_block).unwrap();
    let b_load = Attribute::load(save_load_data).unwrap();
    assert_eq!(b, b_load);
}

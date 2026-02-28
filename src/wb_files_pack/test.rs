use crate::wb_files_pack::{
    DataPosList, PackFileMetadata, PackFileMetadataDir, PackFileMetadataFile, PackFileMetadataType,
    PackStruct, PackStructItem, PackStructItemDir, PackStructItemType,
};

#[test]
fn pack_struct_to_bytes_vec_and_load() {
    let mut ps = PackStruct::default();
    ps.items.insert(
        "test".to_string(),
        PackStructItem {
            name: "test".to_string(),
            metadata_file_pos: 586763,
            item_type: PackStructItemType::Dir(PackStructItemDir {
                struct_file_pos: 289412894,
                pack_struct: None,
            }),
        },
    );
    ps.items.insert(
        "test2".to_string(),
        PackStructItem {
            name: "test2".to_string(),
            metadata_file_pos: 2941,
            item_type: PackStructItemType::Dir(PackStructItemDir {
                struct_file_pos: 29846981,
                pack_struct: None,
            }),
        },
    );
    let data = ps.get_bytes_vec();
    let ps_load = PackStruct::load(0, &data).unwrap();
    assert_eq!(ps, ps_load);
}
#[test]
fn pack_struct_item_to_bytes_vec_and_load() {
    let psi = PackStructItem {
        item_type: PackStructItemType::Dir(PackStructItemDir::default()),
        ..PackStructItem::default()
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
        modified: 571531,
        file_type: PackFileMetadataType::File(PackFileMetadataFile {
            hash_type: 0,
            hash: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
            data_pos_list: DataPosList {
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

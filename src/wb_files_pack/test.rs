use super::data::{MANIFEST_DATA_BLOCK_DATA_VER_INDEX, MANIFEST_DATA_BLOCK_DATA_VER_LEN};
use crate::wb_files_pack::{
    Attribute,
    DataPosList,
    ManifestDataBlock,
    ManifestDataBlockTrait,
    PackFileMetadata,
    PackFileMetadataRun,
    PackFileMetadataType,
    PackStruct,
    PackStructItem,
    PackStructItemType,
    DATA_BLOCK_LEN,
};
use pretty_assertions::assert_eq;

#[test]
fn pack_struct_to_bytes_vec_and_load() {
    let mut ps = PackStruct::default();
    ps.add_item(
        "test".to_string(),
        PackStructItem::new(
            "test".to_string(),
            PackStructItemType::Dir {
                struct_file_pos: 2894,
                pack_struct: None,
            },
            5867,
            PackFileMetadataRun::NoLoad,
        ),
    );
    ps.add_item(
        "test2".to_string(),
        PackStructItem::new(
            "test2".to_string(),
            PackStructItemType::Dir {
                struct_file_pos: 2984,
                pack_struct: None,
            },
            2941,
            PackFileMetadataRun::NoLoad,
        ),
    );
    ps.add_item(
        "test3".to_string(),
        PackStructItem::new(
            "test3".to_string(),
            PackStructItemType::File { handle: None },
            12445,
            PackFileMetadataRun::NoLoad,
        ),
    );
    for index in 0..1000 {
        let name = format!("file{index}");
        ps.add_item(
            name.clone(),
            PackStructItem::new(
                name,
                if index % 2 == 0 {
                    PackStructItemType::Dir {
                        struct_file_pos: rand::random_range(0..100_000_000),
                        pack_struct: None,
                    }
                } else {
                    PackStructItemType::File { handle: None }
                },
                rand::random_range(0..100_000_000),
                PackFileMetadataRun::NoLoad,
            ),
        );
        ps.to_bytes_vec();
    }
    let block_data = ManifestDataBlock::from_block_data_new(ps.get_block_data().0, 0).unwrap();
    let ps_load = PackStruct::load(block_data).unwrap();
    assert_eq!(ps, ps_load);
}
#[test]
fn pack_struct_item_dir_to_bytes_vec_and_load() {
    let psi = PackStructItem::new(
        String::new(),
        PackStructItemType::Dir {
            struct_file_pos: 0,
            pack_struct: None,
        },
        7_766_735_636,
        PackFileMetadataRun::NoLoad,
    );
    let data = psi.to_bytes_vec();
    let psi_load = PackStructItem::load(&data).unwrap();
    assert_eq!(psi, psi_load);
}
#[test]
fn pack_struct_item_file_to_bytes_vec_and_load() {
    let psi = PackStructItem::new(
        String::new(),
        PackStructItemType::File { handle: None },
        689_669,
        PackFileMetadataRun::NoLoad,
    );
    let data = psi.to_bytes_vec();
    let psi_load = PackStructItem::load(&data).unwrap();
    assert_eq!(psi, psi_load);
    let psi_load_data = psi_load.to_bytes_vec();
    assert_eq!(data, psi_load_data);
}
#[test]
fn pack_file_metadata_file_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata::new(false, 53124, 5715, PackFileMetadataType::File {
        hash_type: 0,
        hash_value: Vec::new(),
        data_pos_list: DataPosList::new(vec![(0, 100), (10, 1), (223, 5890)]),
    });
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    assert_eq!(pfm, pfm_load);
    //2
    let hash_value = vec![
        1,
        2,
        3,
        4,
        5,
        6,
        7,
        8,
        9,
        10,
        11,
        12,
        13,
        14,
        15,
        16,
        17,
        18,
        19,
        20,
        21,
        22,
        23,
        24,
        25,
        26,
        27,
        28,
        29,
        30,
        31,
        32
    ];
    let mut pfm2 = pfm.clone();
    if let PackFileMetadataType::File { hash_type, hash_value: hv, .. } = pfm2.file_type_mut() {
        *hash_type = 1;
        *hv = hash_value.clone();
    }
    if
    let PackFileMetadataType::File {
        hash_type,
        hash_value: this_hash_value,
        ..
    } = pfm.file_type_mut()
    {
        *hash_type = 1;
        *this_hash_value = hash_value;
    }
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    pfm2.get_block_data();
    assert_eq!(pfm2, pfm_load);
}
#[test]
fn pack_file_metadata_dir_to_bytes_vec_and_load() {
    let mut pfm = PackFileMetadata::new(true, 52035, 294, PackFileMetadataType::Dir {
        file_count: 0,
        dir_count: 0,
    });
    let block_data = ManifestDataBlock::from_block_data_new(pfm.get_block_data().0, 0).unwrap();
    let pfm_load = PackFileMetadata::load(block_data).unwrap();
    assert_eq!(pfm, pfm_load);
}

#[test]
fn pack_file_metadata_data_block_save_and_load() {
    let mut a = Attribute::default();
    let a_block_data = a.get_block_data().0;
    let a_data_block = ManifestDataBlock::from_block_data_new(a_block_data, 0).unwrap();
    let a_load = Attribute::load(a_data_block).unwrap();
    assert_eq!(a, a_load);
    //Save2
    let mut b = Attribute::default();
    b.set_cow(true);
    b.add_file_count(99);
    b.add_dir_count(877);
    b.add_data_len(231);
    b.set_root_struct_pos(231);
    b.set_empty_data_pos_list_pos(255);
    b.set_manifest_empty_data_pos_list_pos(241);
    b.set_manifest_file_len(123);
    let b_block_data = b.get_block_data().0;
    //Load
    let b_load = Attribute::load(
        ManifestDataBlock::from_block_data_new(b_block_data, 0).unwrap()
    ).unwrap();
    assert_eq!(b, b_load);
    //Save3
    let b_block_data = b.get_block_data().0;
    //Load
    let b_load = Attribute::load(
        ManifestDataBlock::from_block_data_new(b_block_data, 0).unwrap()
    ).unwrap();
    assert_eq!(b, b_load);
}

// ============================================================
// A/B 双块容错回退测试 / A/B dual-block fallback tests
// ============================================================

/// 准备一个经过两次 update 的 ManifestDataBlock（A=ver1, B=ver2）
/// Prepare a ManifestDataBlock with two updates (A=ver1, B=ver2)
fn prepare_ab_block(data_v1: &[u8], data_v2: &[u8]) -> ManifestDataBlock {
    let mut md = ManifestDataBlock::default();
    md.update(data_v1); // A=ver1
    md.update(data_v2); // B=ver2
    md
}

/// 损坏 B 块的尾部版本号 / Corrupt B half's tail version
fn corrupt_b_tail_ver(md: &mut ManifestDataBlock) {
    let block_len = md.block_data().len();
    md.block_data_mut()[block_len - MANIFEST_DATA_BLOCK_DATA_VER_LEN] ^= 0xff;
}

/// 损坏 A 块的尾部版本号 / Corrupt A half's tail version
fn corrupt_a_tail_ver(md: &mut ManifestDataBlock) {
    let half = md.block_data().len() / 2;
    md.block_data_mut()[half - MANIFEST_DATA_BLOCK_DATA_VER_LEN] ^= 0xff;
}

/// 损坏 B 块的头部版本号 / Corrupt B half's head version
fn corrupt_b_head_ver(md: &mut ManifestDataBlock) {
    let half = md.block_data().len() / 2;
    md.block_data_mut()[half + MANIFEST_DATA_BLOCK_DATA_VER_INDEX] ^= 0xff;
}

/// 损坏 A 块的头部版本号 / Corrupt A half's head version
fn corrupt_a_head_ver(md: &mut ManifestDataBlock) {
    md.block_data_mut()[MANIFEST_DATA_BLOCK_DATA_VER_INDEX] ^= 0xff;
}

#[test]
fn ab_b_corrupted_higher_ver_reads_a() {
    // Bug #1 修复验证: B 损坏(ver较高)时回退读取 A
    // Verifies Bug #1 fix: when B is corrupted (higher ver), fallback to A
    let data1 = b"data_version_one_content";
    let data2 = b"data_version_two_newer_content";
    let mut md = prepare_ab_block(data1, data2);

    // 确认当前读到的是 B 的数据 (ver=2 > ver=1)
    assert_eq!(md.get_this_data().unwrap(), data2);

    // 损坏 B 块
    corrupt_b_tail_ver(&mut md);

    // 应回退读取 A 的数据
    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, data1);
}

#[test]
fn ab_a_corrupted_lower_ver_reads_b() {
    // A 损坏时直接读取 B
    // When A is corrupted, should read B directly
    let data1 = b"aaaaaaaaaaaaaaaaaaaa";
    let data2 = b"bbbbbbbbbbbbbbbbbbbb";
    let mut md = prepare_ab_block(data1, data2);

    corrupt_a_tail_ver(&mut md);

    // 应读取 B (ver=2)
    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, data2);
}

#[test]
fn ab_a_corrupted_higher_ver_reads_b() {
    // A 损坏(ver=3), B 完好(ver=2) → A 损坏后直接读 B
    // A corrupted (ver=3), B valid (ver=2) → read B directly
    let data1 = b"first_update_data";
    let data2 = b"second_update_data";
    let data3 = b"third_update_data";
    let mut md = ManifestDataBlock::default();
    md.update(data1); // A=1
    md.update(data2); // B=2
    md.update(data3); // A=3 (higher)

    assert_eq!(md.get_this_data().unwrap(), data3);

    corrupt_a_tail_ver(&mut md);

    // A 损坏 → 应回退读取 B (ver=2)
    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, data2);
}

#[test]
fn ab_both_corrupted_error() {
    // 两个块都损坏时应返回错误
    // Both blocks corrupted should return error
    let data1 = b"xxxx";
    let data2 = b"yyyy";
    let mut md = prepare_ab_block(data1, data2);

    corrupt_a_head_ver(&mut md);
    corrupt_b_head_ver(&mut md);

    let result = md.get_this_data();
    assert!(result.is_err());
}

#[test]
fn ab_both_valid_selects_higher_ver() {
    // 两块都完好时选择版本更高的
    // Both valid → select higher version
    let data1 = b"lower_version_data";
    let data2 = b"higher_version_data";
    let mut md = prepare_ab_block(data1, data2);

    // B 更新，应读到 data2
    assert_eq!(md.get_this_data().unwrap(), data2);

    let data3 = b"even_higher_version_data";
    md.update(data3); // A=3

    // A 更新，应读到 data3
    assert_eq!(md.get_this_data().unwrap(), data3);
}

#[test]
fn ab_from_block_data_new_preserves_fallback() {
    // 验证 from_block_data_new 序列化→反序列化后回退仍然有效
    // Verifies fallback still works after serialization round-trip
    let data1 = b"roundtrip_data_v1";
    let data2 = b"roundtrip_data_v2";
    let mut md = prepare_ab_block(data1, data2);

    corrupt_b_tail_ver(&mut md);

    // 序列化再反序列化 / Serialize then deserialize
    let block_data = md.block_data().to_vec();
    let md2 = ManifestDataBlock::from_block_data_new(block_data, 0).unwrap();

    let recovered = md2.get_this_data().unwrap();
    assert_eq!(recovered, data1);
}

// ============================================================
// 版本号回环测试 / Version wrap-around tests
// ============================================================

#[test]
fn next_ver_normal() {
    assert_eq!(ManifestDataBlock::next_ver(1), 2);
    assert_eq!(ManifestDataBlock::next_ver(5), 6);
    assert_eq!(ManifestDataBlock::next_ver(100), 101);
}

#[test]
fn next_ver_wraps_at_max() {
    // u32::MAX 回环到 1，不得使用 0
    // u32::MAX wraps to 1, 0 is reserved
    assert_eq!(ManifestDataBlock::next_ver(u32::MAX), 1);
    assert_eq!(ManifestDataBlock::next_ver(u32::MAX - 1), u32::MAX);
}

#[test]
fn ver_is_older_normal_range() {
    assert!(ManifestDataBlock::ver_is_older(1, 2));
    assert!(ManifestDataBlock::ver_is_older(0, 1));
    assert!(!ManifestDataBlock::ver_is_older(2, 1));
    assert!(!ManifestDataBlock::ver_is_older(5, 5));
}

#[test]
fn ver_is_older_wrap_around() {
    // MAX 之后回环到 1，MAX 应该比 1 旧
    // After MAX wraps to 1, MAX should be older than 1
    assert!(ManifestDataBlock::ver_is_older(u32::MAX, 1));
    assert!(!ManifestDataBlock::ver_is_older(1, u32::MAX));

    // 回环后的版本比接近 MAX 的版本更新
    // Post-wrap versions are newer than near-MAX versions
    assert!(ManifestDataBlock::ver_is_older(u32::MAX - 1, 2));
    assert!(!ManifestDataBlock::ver_is_older(2, u32::MAX - 1));

    // 回环后两个版本比较
    // Comparing two post-wrap versions
    assert!(ManifestDataBlock::ver_is_older(1, 5));
    assert!(ManifestDataBlock::ver_is_older(10, 20));
}

#[test]
fn get_ver_rejects_zero() {
    // 版本 0 应被视为损坏 (头尾都=0)
    // Version 0 should be treated as corrupted (head=tail=0)
    let block_size = DATA_BLOCK_LEN * 2; // 256 bytes minimum
    let data = vec![0u8; block_size];
    // 头尾 ver 都是 0 → get_ver 返回 Err
    let result = ManifestDataBlock::get_ver(&data);
    assert!(result.is_err(), "get_ver should reject version 0");
}

#[test]
fn get_ver_rejects_zero_when_head_tail_match() {
    // 即使头尾版本匹配(=0)，也应被拒绝
    // Even when head and tail versions match (=0), should be rejected
    let block_size = DATA_BLOCK_LEN * 2;
    let data = vec![0u8; block_size];
    // 头 ver=0 (already 0)
    // 尾 ver=0 (already 0)
    // 都匹配，但值为 0 → 应拒绝
    let result = ManifestDataBlock::get_ver(&data);
    assert!(result.is_err());
}

#[test]
fn get_ver_accepts_nonzero() {
    // 非零版本号应该通过
    let block_size = DATA_BLOCK_LEN * 2;
    let mut data = vec![0u8; block_size];
    let ver: u32 = 42;
    let ver_bytes = ver.to_le_bytes();
    // 设置头部版本
    data[
        MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_bytes);
    // 设置尾部版本
    let tail_start = block_size - MANIFEST_DATA_BLOCK_DATA_VER_LEN;
    data[tail_start..tail_start + MANIFEST_DATA_BLOCK_DATA_VER_LEN].copy_from_slice(&ver_bytes);

    let result = ManifestDataBlock::get_ver(&data).unwrap();
    assert_eq!(result, 42);
}

#[test]
fn update_wraps_version_near_max() {
    // 模拟版本接近 u32::MAX 时的 update 回环
    // Simulate update near u32::MAX to verify wrap-around
    let data1 = b"data_at_max_version";
    let data2 = b"data_at_wrapped_version";

    let mut md = ManifestDataBlock::default();
    md.update(data1); // A=ver=1

    // 手动将 B 块的版本设置为 u32::MAX-1，A 设置为 u32::MAX
    // 这样下次 update 会选中 A (ver较低.. 其实是 ver=u32::MAX，B 是 ver=u32::MAX-1)
    // 等等: ver_is_older(u32::MAX-1, u32::MAX) = true，所以 B 更旧
    // 下次 update 会更新 B: ver = next_ver(u32::MAX) = 1
    let half = md.block_data().len() / 2;

    // 设置 A (前半) 版本为 u32::MAX
    let ver_max = u32::MAX.to_le_bytes();
    md.block_data_mut()[
        MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_max);
    md.block_data_mut()[half - MANIFEST_DATA_BLOCK_DATA_VER_LEN..half].copy_from_slice(&ver_max);

    // 设置 B (后半) 版本为 u32::MAX - 1
    let ver_max_minus_1 = (u32::MAX - 1).to_le_bytes();
    let block_len = md.block_data().len();
    md.block_data_mut()[
        half + MANIFEST_DATA_BLOCK_DATA_VER_INDEX..half +
            MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_max_minus_1);
    md.block_data_mut()[block_len - MANIFEST_DATA_BLOCK_DATA_VER_LEN..].copy_from_slice(
        &ver_max_minus_1
    );

    // B(MAX-1) 比 A(MAX) 更旧 → 更新 B 为 next_ver(MAX) = 1
    md.update(data2);

    // 现在 B 应该 ver=1，应读取 data2
    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, data2);

    // 再次 update 验证能继续正常交替
    let data3 = b"data_after_wrap";
    md.update(data3);
    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, data3);
}

#[test]
fn update_wraps_at_boundary_then_continues() {
    // 直接在回环边界连续更新: MAX→1→2→3 验证正常交替
    // Update across wrap boundary: MAX→1→2→3, verify normal alternation
    let mut md = ManifestDataBlock::default();
    md.update(b"step0");

    let half = md.block_data().len() / 2;

    // 设置 A 版本为 u32::MAX
    let ver_max = u32::MAX.to_le_bytes();
    md.block_data_mut()[
        MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_max);
    md.block_data_mut()[half - MANIFEST_DATA_BLOCK_DATA_VER_LEN..half].copy_from_slice(&ver_max);

    // B 处于 ver=2 (第二次 update 的结果)
    // 手动读取 B 当前版本以确认
    // 实际上经过 default → update(step0): A=1; update(step1=B here? No, only 1 update)
    // 我们需要两次 update 才有 A=1, B=2
    md.update(b"step1"); // 这次 update 实际上是在 step0 之后: A=1, 然后 update step1 → B=2

    // 重新设置 A 版本为 u32::MAX
    md.block_data_mut()[
        MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_max);
    md.block_data_mut()[half - MANIFEST_DATA_BLOCK_DATA_VER_LEN..half].copy_from_slice(&ver_max);

    // B 是 ver=2 (较新), A 是 ver=MAX (较旧) → 更新 A: next_ver(MAX) = 1
    // 等等: ver_is_older(u32::MAX, 2)?
    // 2.wrapping_sub(MAX) = 3
    // MAX.wrapping_sub(2) = MAX-2 ≈ 4 billion
    // 3 < 4 billion → MAX is older! Correct.
    md.update(b"step2_wrap");

    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, b"step2_wrap");

    // 继续正常更新 / Continue normal updates
    md.update(b"step3");
    assert_eq!(md.get_this_data().unwrap(), b"step3");

    md.update(b"step4");
    assert_eq!(md.get_this_data().unwrap(), b"step4");
}

#[test]
fn ab_fallback_works_after_wrap_around() {
    // 回环后 A/B 回退仍正常工作
    // A/B fallback still works after wrap-around
    let data_a = b"after_wrap_data_a";
    let data_b = b"after_wrap_data_b";

    let mut md = ManifestDataBlock::default();
    md.update(data_a);

    let half = md.block_data().len() / 2;
    // 设置 A 版本为 MAX-1
    let ver_max_m1 = (u32::MAX - 1).to_le_bytes();
    md.block_data_mut()[
        MANIFEST_DATA_BLOCK_DATA_VER_INDEX..MANIFEST_DATA_BLOCK_DATA_VER_INDEX +
            MANIFEST_DATA_BLOCK_DATA_VER_LEN
        ].copy_from_slice(&ver_max_m1);
    md.block_data_mut()[half - MANIFEST_DATA_BLOCK_DATA_VER_LEN..half].copy_from_slice(&ver_max_m1);

    // B 初始为空 (ver=0, 会被拒绝)
    // 所以 update 会选中 B: next_ver(MAX-1) = MAX
    md.update(data_b); // B=ver=MAX-1+1=MAX, 或 next_ver(MAX-1)=MAX

    // 现在 A=MAX-1, B=MAX
    // 再次 update: ver_is_older(MAX-1, MAX)=true → 更新 A: next_ver(MAX)=1
    md.update(b"newest_data");

    let recovered = md.get_this_data().unwrap();
    assert_eq!(recovered, b"newest_data");

    // 损坏 B (ver=MAX) → 应回退到 A (ver=1)
    corrupt_b_tail_ver(&mut md);

    let recovered = md.get_this_data().unwrap();
    // A 在 wrap 后被更新为 "newest_data" (ver=1)
    assert_eq!(recovered, b"newest_data");
}

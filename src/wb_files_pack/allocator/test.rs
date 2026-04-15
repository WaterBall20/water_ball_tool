use crate::wb_files_pack::allocator::Allocator;

//TEST===
static WBFP_TEST_TEMP_OK_DIR_PATH: &str = "./temp/test/wbfp/ok";
static _WBFP_TEST_TEMP_ERR_DIR_PATH: &str = "./temp/test/wbfp/err";
//哈希校验===
// 长时间
#[test]
#[ignore = "长时间"]
fn wbfp_verify_all_file_hash_longtime() {
    let mut in_dir_path = String::from(WBFP_TEST_TEMP_OK_DIR_PATH);
    in_dir_path.push_str("/create_new_pack_m");
    let mut in_file_path = in_dir_path.clone();
    in_file_path.push_str("/pack");
    let mut allocator =
        Allocator::open_pack_file(&in_file_path).expect("测试打开包文件失败");
    let hash_e = allocator.verify_all_file_hash();
    println!("{hash_e:#?}");
}

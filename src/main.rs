use std::env;
use WaterBallTool::{file_finder, wb_files_pack};
// 开始时间:2026-02-08 22:37

const APP_NAME: &str = "WaterBallTool";

fn main() {
    //命令行参数处理
    let args: Vec<String> = env::args().collect();
    if let Some(mod_type) = args.get(2) {
        match &mod_type[..] {
            "ff" => {
                file_finder::command(&args);
            }
            _ => {}
        }
    }
    /* args.push(String::from("ff"));
     args.push(String::from("/home/waterball/Downloads"));
     args.push(String::from("temp/ff_test_json"));
     args.push(String::from("-s"));
     file_finder::command(&args)*/

    let wb_files_pack = wb_files_pack::manager::create_file2(&"temp/test.wbfp", true, true);
    if let Ok(mut wb_files_pack) = wb_files_pack {
        wb_files_pack.init_pack();
    }
}

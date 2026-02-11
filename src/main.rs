mod file_finder;
mod wb_files_pack;

use std::env;
// 开始时间:2026-02-08 22:37

const APP_NAME : &str = "WaterBallTool";

fn main() {
    //命令行参数处理
    let mut args : Vec<String> = env::args().collect();
    if let Some(mod_type) = args.get(2) {
        match &mod_type[..] {
            "ff" => {
                file_finder::command(&args);
            }
            _ => {}
        }
    }
    args.push(String::from("ff"));
    args.push(String::from("/home/waterball"));
    args.push(String::from("ff_test_json"));
    args.push(String::from("-s"));
    file_finder::command(&args)
}

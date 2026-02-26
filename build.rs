use std::env;

fn main() {
    // 获取 Cargo.toml 中的版本号
    let version = env::var("CARGO_PKG_VERSION").unwrap();

    // 如果版本号包含 "beta"，则向编译器发送自定义标签
    if version.contains("beta") || version.starts_with("0.9") {
        println!("cargo:rustc-cfg=is_beta");
    }

    // 你甚至可以根据具体的版本号段来控制
    if version.starts_with("1.0") {
        println!("cargo:rustc-cfg=version_v1");
    }
}

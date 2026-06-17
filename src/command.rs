/*
创建时间:26/02/24 80:40
*/
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, io, thread};
use tracing::{error, info, warn};
use water_ball_tool::file_finder::{FileFinder, FileInfo, FileKind, FilesList};
use water_ball_tool::wb_files_pack::allocator::Allocator;
use water_ball_tool::wb_files_pack::{
    PackFileMetadata, PackFileMetadataRun, PackStructItem, PackStructItemType,
};

#[cfg(test)]
mod test;

static BUF_LEN: usize = 1024 * 1024;

const PROGRESS_STYLE_TEMPLATE: &str =
    "{spinner:.green} [{elapsed_precise}({eta})] [{bar:40.cyan/blue}] {msg:>7}";

//文件查找器
pub fn ff(args: &[String], mp: Option<&MultiProgress>) {
    //参数格式：[指定搜索路径,输出路径,跳过符号链接]
    //获取参数中的指定的路径，若没有则使用程序路径
    let path = match args.first() {
        Some(value) => value,
        None => ".",
    };

    //跳过符号链接参数（暂用）
    let skip_symlink = match args.get(2) {
        Some(value) => value.contains("-s"),
        None => false,
    };
    //进度条
    let pb = create_pb(mp);

    let files_list = search_files(path, skip_symlink, Option::from(&pb)).unwrap();

    //输出到输出文件(若存在参数)
    if let Some(out_path) = args.get(1) {
        //只写模式打开文件，
        match File::create(out_path) {
            Ok(mut f) => {
                info!("正在将搜索结果输出到文件");
                let data = serde_json::to_vec_pretty(&files_list).expect("数据转换错误");
                f.write_all(&data).expect("保存到文件错误");
                info!(r#"搜索结果已输出到文件: "{out_path}""#);
            }
            Err(err) => {
                panic!(r#"无法打开输出文件: "{out_path}" , Error: '{err}'"#)
            }
        }
    } else {
        info!("搜索结果：{files_list:?}");
    } /**/
}

//创建进度实例
fn create_pb(mp: Option<&MultiProgress>) -> Option<ProgressBar> {
    if let Some(mp) = mp {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.enable_steady_tick(Duration::from_millis(100)); // 让转标自己动起来
        Some(pb)
    } else {
        None
    }
}
//设置进度样式：显示进度条
fn set_pb_style2(pb: Option<&ProgressBar>, data_len: u64) {
    if let Some(pb) = pb {
        pb.set_length(data_len);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(PROGRESS_STYLE_TEMPLATE)
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_message("0.00%");
    }
}

fn update_pb(
    pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
    path1: &Path,
    path2: &Path,
    lase_up_pb_c_write_len: &mut u64,
    write_len: &mut u64,
) {
    if let Some(pb_c) = pb_c {
        let l_len = *write_len - *lase_up_pb_c_write_len;
        if l_len > 10 * (BUF_LEN as u64) {
            pb_c(
                l_len,
                0,
                path1.display().to_string(),
                path2.display().to_string(),
            );
            *lase_up_pb_c_write_len = *write_len;
        }
    }
}

const SEARCH_MAX_THREAD_COUNT: usize = 64;

/// 搜索文件并显示进度 / Search files with progress display
fn search_files(path: &str, skip_symlink: bool, pb: Option<&ProgressBar>) -> io::Result<FilesList> {
    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} ({pos} 个文件和目录)")
                .unwrap(),
        );
        pb.set_message("搜索文件中");
    }
    let mut file_count = 0;
    let mut dir_count = 0;

    //搜索
    let ff = FileFinder;
    if let Some(pb) = &pb {
        let (tx, rx) = mpsc::channel();
        let (rtx, rrx) = mpsc::channel();
        let t_path = path.to_string();
        thread::spawn(move || {
            rtx.send(ff.search(t_path.as_ref(), skip_symlink, tx, SEARCH_MAX_THREAD_COUNT))
                .expect("线程发送结果错误");
        });
        for (add_file, add_dir) in rx {
            file_count += add_file;
            dir_count += add_dir;
            let all_count = file_count + dir_count;
            pb.set_position(all_count);
            //10的倍数才更新
            if all_count.is_multiple_of(10) {
                pb.set_message(format!("已发现 {file_count} 文件和 {dir_count} 个目录"));
            }
        }
        rrx.recv().expect("无法获取结果")
    } else {
        let (tx, _) = mpsc::channel();
        ff.search(path.as_ref(), skip_symlink, tx, SEARCH_MAX_THREAD_COUNT)
    }
}

//水球包文件
pub fn wbfp(args: &[String], mp: Option<&MultiProgress>) {
    let arg = args.first().expect("参数不足");
    match arg.as_str() {
        "-s" => wbfp_s(&args[1..], mp),
        "-m" => wbfp_m(&args[1..], mp),
        "-h" => wbfp_h(&args[1..], mp),
        _ => panic!(
            "未知的路由参数: {arg}\\
    提示：
        -s  :  解包文件 | <包文件路径> <输出目录>
        -m  :  打包文件 | <输入目录> <包文件路径> [-f]
                        -f  :   不分离数据到单独的文件
        -h  :  哈希校验 | <包文件路径>
"
        ),
    }
}

//水球包文件打包
pub fn wbfp_m(args: &[String], mp: Option<&MultiProgress>) {
    //参数格式：[源文件目录,目标文件路径,分离数据文件,写时复制]
    assert!(
        args.len() >= 2,
        "参数数量不够，至少需要 <输入目录> <目标路径>"
    );
    //源目录路径
    let in_dir_path = &args[0];
    //输出的包文件路径
    let pack_path = &args[1];
    //分离数据文件
    let separate_manifest = match args.get(2) {
        Some(value) => {
            if value.contains("-f") {
                !water_ball_tool::wb_files_pack::manager::DEFAULT_SEPARATE_MANIFEST
            } else {
                water_ball_tool::wb_files_pack::manager::DEFAULT_SEPARATE_MANIFEST
            }
        }
        None => water_ball_tool::wb_files_pack::manager::DEFAULT_SEPARATE_MANIFEST,
    };

    //进度条
    let pb = create_pb(mp);
    info!("开始准备打包");
    info!("创建新包文件并初始化");
    let mut pack =
        Allocator::create_new_pack_file(&pack_path, false, separate_manifest).expect("创建包文件错误");
    //逻辑实现=== ===
    //搜索文件===
    info!("搜索文件");
    warn!("目前搜索将跳过符号链接");
    let files_list = search_files(in_dir_path, true, Option::from(&pb)).unwrap();
    //包文件===
    if let Some(pb) = &pb {
        let total_files = files_list.data_length();
        pb.set_length(total_files);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(PROGRESS_STYLE_TEMPLATE)
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_message("0.00%");
    }
    info!("开始复制数据");
    write_pack(
        &mut pack,
        Option::from(&pb),
        &files_list,
        in_dir_path.as_ref(),
    )
        .expect("写入包文件错误");
    info!("操作已完成,文件保存到{pack_path}");
}
fn write_pack(
    pack_man: &mut Allocator,
    pb: Option<&ProgressBar>,
    files_list: &FilesList,
    in_dir_path: &Path,
) -> io::Result<()> {
    /// 递归将文件列表写入包 / Recursively write file list into pack
    fn write_pack_recursive<'a>(
        pack: &mut Allocator,
        mut pb_c: Option<&'a mut (dyn FnMut(u64, u64, String, String) + 'a)>,
        info_list: &HashMap<String, FileInfo>,
        in_s_path_buf: &Path,
        pack_s_path_buf: &Path,
        run_buf: &mut [u8],
    ) -> io::Result<Option<&'a mut dyn FnMut(u64, u64, String, String)>> {
        for (name, info) in info_list {
            let this_in_path = in_s_path_buf.join(name);
            let this_pack_path = pack_s_path_buf.join(name);
            match info.file_kind() {
                FileKind::File => copy_file_into_pack(
                    pack,
                    &mut pb_c,
                    run_buf,
                    info,
                    &this_in_path,
                    &this_pack_path,
                ),
                FileKind::Dir(dir) => {
                    //目录仅递归处理
                    pb_c = write_pack_recursive(
                        pack,
                        pb_c,
                        dir.files_list(),
                        &this_in_path,
                        &this_pack_path,
                        run_buf,
                    )?;
                }
            }
        }
        Ok(pb_c)
    }

    let mut buf = vec![0u8; BUF_LEN];
    let mut this_all_write_len = 0;
    let mut this_all_write_file_count = 0;
    let all_file_count = files_list.file_count();
    let mut binding = |add_len, add_file_count, f_path: String, p_path: String| {
        this_all_write_len += add_len;
        this_all_write_file_count += add_file_count;
        if let Some(pb) = pb {
            pb.set_position(this_all_write_len);
            let percent = ((this_all_write_len as f64) / (files_list.data_length() as f64)) * 100.0;
            pb.set_message(format!(
                "[{percent:>6.2}%][{this_all_write_file_count}/{all_file_count}个文件] \
                \nFile path: {f_path}\
                \nPack path: {p_path}"
            ));
        }
    };
    write_pack_recursive(
        pack_man,
        match pb {
            Some(_) => Some(&mut binding),
            None => None,
        },
        files_list.files_list(),
        in_dir_path,
        &PathBuf::new(),
        &mut buf,
    )?;
    Ok(())
}

fn copy_file_into_pack(
    pack_man: &mut Allocator,
    pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
    run_buf: &mut [u8],
    info: &FileInfo,
    this_in_path: &PathBuf,
    this_pack_path: &PathBuf,
) {
    //更新进度
    let mut lase_up_pb_c_write_len = 0;
    if let Some(pb_c) = pb_c {
        pb_c(
            0,
            1,
            this_in_path.display().to_string(),
            this_pack_path.display().to_string(),
        );
    }
    //尝试打开文件
    let mut in_file = match File::open(this_in_path) {
        Ok(file) => file,
        Err(err) => {
            match err.kind() {
                ErrorKind::PermissionDenied => {
                    //权限不足
                    error!("无法打开文件{this_in_path:?}，权限不足，将跳过，err:{err}");
                }
                _ => panic!(
                    r#"[未处理错误]无法打开文件"{}"，err:{err}"#,
                    this_in_path.display()
                ),
            }
            return;
        }
    };
    //尝试创建虚拟文件
    let mut out_file =
        match pack_man.create_file(this_pack_path, info.modified_time(), info.length()) {
            Ok(v) => v,
            Err(err) => {
                error!(
                    "无法创建虚拟文件{},将跳过, err:{err}",
                    this_pack_path.display()
                );
                return;
            }
        };
    //写入操作
    let mut write_len = 0;
    while write_len < info.length() {
        //读
        match in_file.read(run_buf) {
            Ok(this_read_len) => {
                match out_file.write(&run_buf[..this_read_len]) {
                    Ok(this_write_len) => {
                        assert_ne!(
                            this_read_len,
                            0,
                            "文件{}读取的大小为0，但于预期不符，文件大小可能不是0",
                            this_pack_path.display()
                        );
                        if this_write_len < this_read_len {
                            warn!(
                                "文件{}写入虚拟文件{}大小不一致，读：{this_read_len}，写：{this_write_len}",
                                this_in_path.display(),
                                this_pack_path.display()
                            );
                        }
                        write_len += this_write_len as u64;
                        //更新进度
                        update_pb(
                            pb_c,
                            this_in_path,
                            this_pack_path,
                            &mut lase_up_pb_c_write_len,
                            &mut write_len,
                        );
                    }
                    Err(err) => {
                        error!("写入虚拟文件{this_pack_path:?}错误, 将跳过，err:{err}");
                        break;
                    }
                }
            }
            Err(err) => {
                error!("读取文件{this_in_path:?}失败，将跳过，err:{err}");
                break;
            }
        }
    }
    //不论是否写入成功都对齐进度条
    if let Some(pb_c) = pb_c {
        pb_c(
            info.length() - write_len,
            0,
            this_in_path.display().to_string(),
            this_pack_path.display().to_string(),
        );
    }
}

//水球包文件解包
pub fn wbfp_s(args: &[String], mp: Option<&MultiProgress>) {
    //参数格式：[源文件目录,目标文件路径,分离数据文件,写时复制]
    assert!(
        args.len() >= 2,
        "参数数量不够，至少需要 <包文件路径> <目标目录>"
    );
    //包文件路径
    let pack_path = &args[0];
    //输出文件路径
    let out_dir_path = &args[1];

    //进度条
    let pb = create_pb(mp);
    info!("开始准备解包");
    info!("打开包文件");
    let mut pack = Allocator::open_pack_file(pack_path).expect("打开包文件错误");
    //逻辑实现=== ===
    info!("开始复制数据");
    fs::create_dir_all(out_dir_path).expect("无法创建数据路径");
    read_pack(&mut pack, Option::from(&pb), out_dir_path.as_ref()).expect("写入文件错误");
    info!("操作已完成,文件保存到目录{out_dir_path}");
}

fn read_pack(
    pack_man: &mut Allocator,
    pb: Option<&ProgressBar>,
    out_dir_path: &Path,
) -> io::Result<()> {
    /// 递归从包读取文件 / Recursively read files from pack
    fn read_pack_recursive<'a>(
        pack_man: &mut Allocator,
        mut pb_c: Option<&'a mut (dyn FnMut(u64, u64, String, String) + 'a)>,
        pack_struct_items: &HashMap<String, PackStructItem>,
        out_s_path_buf: &Path,
        pack_s_path_buf: &Path,
        run_buf: &mut [u8],
    ) -> io::Result<Option<&'a mut dyn FnMut(u64, u64, String, String)>> {
        for (name, item) in pack_struct_items {
            let this_out_path = out_s_path_buf.join(name);
            let this_pack_path = pack_s_path_buf.join(name);
            match item.item_type() {
                PackStructItemType::File => {
                    if let PackFileMetadataRun::Loaded(metadata) = item.metadata() {
                        copy_pack_file_to_disk(
                            pack_man,
                            &mut pb_c,
                            run_buf,
                            metadata,
                            &this_out_path,
                            &this_pack_path,
                        );
                    } else {
                        error!("虚拟路径{this_pack_path:?}文件的元数据没有被加载");
                    }
                }
                PackStructItemType::Dir { pack_struct, .. } => {
                    if let Some(pack_struct) = pack_struct {
                        //尝试获取结构项
                        //创建目录
                        fs::create_dir_all(&this_out_path)?;
                        //目录仅递归处理
                        pb_c = read_pack_recursive(
                            pack_man,
                            pb_c,
                            pack_struct.items(),
                            &this_out_path,
                            &this_pack_path,
                            run_buf,
                        )?;
                    } else {
                        error!("虚拟路径{this_pack_path:?}目录的结构没有被加载");
                    }
                }
            }
        }
        Ok(pb_c)
    }

    //
    info!("加载所有结构项和元数据");
    pack_man.load_all_data(false)?;
    info!("加载完成");

    //获取根列表
    let root_struct_list = pack_man.get_root_struct_items()?;
    let mut buf = vec![0; BUF_LEN];
    let mut this_all_write_len = 0;
    let mut this_all_write_file_count = 0;
    let attribute = pack_man.get_manifest_attribute()?;
    let all_file_count = attribute.file_count();
    let data_len = attribute.data_len();
    //设置进度条样式
    set_pb_style2(pb, data_len);
    let mut binding = |add_len, add_file_count, f_path, p_path| {
        this_all_write_len += add_len;
        this_all_write_file_count += add_file_count;
        if let Some(pb) = pb {
            pb.set_position(this_all_write_len);
            let percent = ((this_all_write_len as f64) / (data_len as f64)) * 100.0;
            pb.set_message(format!(
                "[{percent:>6.2}%][{this_all_write_file_count}/{all_file_count}个文件]\
                \nPack path: {p_path}\
                \nFile path: {f_path}"
            ));
        }
    };
    read_pack_recursive(
        pack_man,
        match pb {
            Some(_) => Some(&mut binding),
            None => None,
        },
        &root_struct_list,
        out_dir_path,
        &PathBuf::new(),
        &mut buf,
    )?;
    Ok(())
}

fn copy_pack_file_to_disk(
    pack_man: &mut Allocator,
    pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
    run_buf: &mut [u8],
    metadata: &PackFileMetadata,
    this_out_path: &PathBuf,
    this_pack_path: &PathBuf,
) {
    //更新进度
    let mut lase_up_pb_c_write_len = 0;
    if metadata.len() > 1024 * 1024 * 512 {
        info!(
            r#"正在从包文件虚拟路径"{}"复制大文件到"{}"，大小:{}[{}]"#,
            this_pack_path.display(),
            this_out_path.display(),
            water_ball_tool::tools::bytes_len_to_string(metadata.len()),
            metadata.len()
        );
    }
    if let Some(pb_c) = pb_c {
        pb_c(
            0,
            1,
            this_pack_path.display().to_string(),
            this_out_path.display().to_string(),
        );
    }
    //尝试打开虚拟文件
    let mut in_file = match pack_man.get_file_wr(this_pack_path, false) {
        Ok(file) => file,
        Err(err) => {
            error!("无法打开虚拟文件{this_pack_path:?}，将跳过，err:{err}");
            return;
        }
    };
    //尝试创建文件
    let mut out_file = match File::create(this_out_path) {
        Ok(file_wr) => file_wr,
        Err(err) => {
            error!("无法创建文件{this_out_path:?},将跳过，err：{err}");
            return;
        }
    };
    //分配空间
    if let Err(err) = out_file.set_len(metadata.len()) {
        warn!("无法对输出文件{this_out_path:?}进行预分配空间，将继续, err:{err}");
    }
    //写入操作
    let mut write_len = 0;
    while write_len < metadata.len() {
        //读
        match in_file.read(run_buf) {
            Ok(this_read_len) => {
                match out_file.write(&run_buf[..this_read_len]) {
                    Ok(this_write_len) => {
                        if this_read_len == 0 {
                            warn!("虚拟文件{this_pack_path:?}读取的大小为0, 将跳过。");
                            break;
                        }
                        if this_write_len != this_read_len {
                            warn!(
                                "虚拟文件{this_pack_path:?}写入文件{this_out_path:?}大小不一致，读：{this_read_len}，写：{this_write_len}"
                            );
                        }
                        write_len += this_write_len as u64;
                        //更新进度
                        update_pb(
                            pb_c,
                            this_pack_path,
                            this_out_path,
                            &mut lase_up_pb_c_write_len,
                            &mut write_len,
                        );
                    }
                    Err(err) => {
                        error!("写入文件{this_pack_path:?}错误, 将跳过，err:{err}");
                        break;
                    }
                }
            }
            Err(err) => {
                error!("读取虚拟文件{this_pack_path:?}失败，将跳过，err:{err}");
                break;
            }
        }
    }
    //不论是否写入成功都对齐进度条
    if let Some(pb_c) = pb_c {
        pb_c(
            metadata.len() - write_len,
            0,
            this_pack_path.display().to_string(),
            this_out_path.display().to_string(),
        );
    }
}
//包文件哈希校验
fn wbfp_h(args: &[String], mp: Option<&MultiProgress>) {
    //包文件路径
    let pack_path = &args[0];

    //进度条
    let pb = create_pb(mp);
    info!("开始准备哈希校验");
    info!("打开包文件");
    let mut pack = Allocator::open_pack_file(pack_path).expect("打开包文件错误");
    //逻辑实现=== ===
    info!("开始哈希校验");
    verify_hash(&mut pack, pb.as_ref()).unwrap();
    info!("操作已完成，没有警告（WARN）或错误（ERROR）说明全部通过。");
}

fn verify_hash(pack: &mut Allocator, pb: Option<&ProgressBar>) -> io::Result<()> {
    info!("加载所有结构项和元数据");
    pack.load_all_data(false)?;
    info!("加载完成");
    let root_name_list = pack.get_root_struct_item_name_list()?;
    let mut this_all_write_len = 0;
    let mut this_all_write_file_count = 0;
    let attribute = pack.get_manifest_attribute()?;
    let all_file_count = attribute.file_count();
    let data_len = attribute.data_len();

    //设置进度条样式
    set_pb_style2(pb, data_len);
    let mut binding = |add_len, add_file_count, p_path| {
        this_all_write_len += add_len;
        this_all_write_file_count += add_file_count;
        if let Some(pb) = pb {
            pb.set_position(this_all_write_len);
            let percent = ((this_all_write_len as f64) / (data_len as f64)) * 100.0;
            pb.set_message(format!(
                "[{percent:>6.2}%][{this_all_write_file_count}/{all_file_count}个文件] \
                \nPack path: {p_path}"
            ));
        }
    };

    for root_name in root_name_list {
        verify_hash_inner(
            pack,
            root_name.as_ref(),
            if pb.is_some() {
                Some(&mut binding)
            } else {
                None
            },
        );
    }
    Ok(())
}

fn verify_hash_inner<'a>(
    pack: &mut Allocator,
    path: &Path,
    mut pb_c: Option<&'a mut (dyn FnMut(u64, u64, String) + 'a)>,
) -> Option<&'a mut dyn FnMut(u64, u64, String)> {
    let item = match pack.get_pack_struct_item(path) {
        Ok(v) => v,
        Err(err) => {
            error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
            return pb_c;
        }
    };
    match item.item_type() {
        PackStructItemType::Dir { .. } => {
            let items_name = match pack.get_struct_item_name_list(path) {
                Ok(v) => v,
                Err(err) => {
                    error!(
                        r#"无法获取虚拟路径"{}"的结构项名称, err:{err}"#,
                        path.display()
                    );
                    return pb_c;
                }
            };
            let mut pb_c = pb_c;
            for name in items_name {
                pb_c = verify_hash_inner(pack, &path.join(name), pb_c);
            }
            pb_c
        }
        PackStructItemType::File => {
            let mut rw = match pack.get_file_wr(path, false) {
                Ok(v) => v,
                Err(err) => {
                    error!("无法获取包文件读写器，err: {err}");
                    return pb_c;
                }
            };
            match rw.verify_hash() {
                Ok(false) => {
                    warn!(r#"虚拟文件"{}"哈希验证失败"#, path.display());
                }
                Err(err) => warn!(
                    r#"虚拟文件"{}"哈希验证发生错误, err: {err:?}"#,
                    path.display()
                ),
                _ => (),
            }
            if let Some(pb) = &mut pb_c {
                pb(rw.get_len(), 1, path.display().to_string());
            }
            pb_c
        }
    }
}

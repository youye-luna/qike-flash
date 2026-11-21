use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// 把 `assets/app.ico` 编译成 Windows 资源对象并链接进可执行文件，
/// 这样 exe 在资源管理器 / 任务栏 / 快捷方式里才显示程序图标。
fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-env-changed=WINDRES");

    if env::var_os("CARGO_CFG_TARGET_OS").as_deref() != Some(OsStr::new("windows")) {
        return;
    }

    // 源码路径含中文，mingw 的 windres 打不开；统一复制到 ASCII 的 OUT_DIR 再处理。
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR 未设置"));
    fs::copy("assets/app.ico", out_dir.join("app.ico")).expect("复制 app.ico 失败");
    fs::write(out_dir.join("app.rc"), "1 ICON \"app.ico\"\n").expect("写入 app.rc 失败");

    let windres = env::var("WINDRES").unwrap_or_else(|_| "windres".to_string());
    let status = Command::new(&windres)
        .current_dir(&out_dir)
        .args(["-O", "coff", "-i", "app.rc", "-o", "app.res.o"])
        .status()
        .unwrap_or_else(|e| panic!("无法执行 {windres}：{e}（可用 WINDRES 环境变量指定路径）"));
    assert!(status.success(), "{windres} 编译资源失败");

    let obj = out_dir.join("app.res.o");
    println!("cargo:rustc-link-arg-bins={}", obj.display());
}
